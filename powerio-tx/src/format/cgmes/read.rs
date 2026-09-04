//! Merged CGMES profiles into a [`BalancedNetwork`].
//!
//! `TopologicalNode` is the bus when the set carries TP data. Terminals tie
//! conducting equipment to nodes (directly in 2.4.15 TP; through
//! `ConnectivityNode.TopologicalNode` in 3.0). Without TopologicalNode data
//! a node breaker set (EquipmentOperation in 2.4.15, any CGMES 3.0 EQ with
//! ConnectivityNodes) has its buses calculated: the connected components of
//! the ConnectivityNode graph joined by closed, in service switches, the same
//! selection PowSybl's `CgmesModelTripleStore.computeIsNodeBreaker` makes.
//! SSH carries the operating point (`p`/`q`, switch position, tap steps,
//! sections); SV supplies solved voltage, tap, and terminal power values.
//! Missing SSH degrades gracefully; vendor exports like the CIGRE MV set
//! ship EQ/TP/SV only, so element `p`/`q` falls back to the terminal's
//! `SvPowerFlow`.
//!
//! CGMES values are MW/MVAr/kV/ohm/S; per-unit lands on a 100 MVA system
//! base. Everything the mapping does not consume is counted per class into
//! the parse warnings, never dropped silently.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use powerio_core::ComponentId;

use super::xml::{CimDocument, CimObject, ModelHeader, PropValue, parse_cimxml};
use super::{CGMES_CLASS_PROPERTY, CgmesDiagnostics, CgmesVersion};
use crate::diagnostics::codes;
use crate::network::{
    AcDcConverterControlMode, ActivePowerControl, BalancedNetwork, Branch, BranchCharging, Bus,
    BusBreakerBus, BusId, BusType, BusbarSection, CalculatedBus, ComponentAlias, ComponentMetadata,
    ConnectivityNode, CurveStyle, DcBusbar, DcConverterOperatingMode, DcConverterUnit, DcGround,
    DcLine, DcNode, DcPolarity, DcSeriesDevice, DcSwitch, DcSwitchKind, DcTerminal,
    DcTopologicalNode, DetailedConnectivity, EquipmentReactiveLimits, ExternalIdentifier,
    Generator, GeneratorEnergySource, Impedance, Junction, LineCommutatedConverter,
    LineCommutatedConverterOperatingMode, Load, LoadVoltageModel, LoadingLimits,
    OperationalLimitGroup, ReactiveCapabilityCurve, ReactiveCapabilityCurvePoint, ReactiveLimits,
    Shunt, ShuntBlock, SourceFormat, StaticVarCompensator, StaticVarCompensatorRegulationMode,
    Substation, Switch, SwitchKind, SwitchedShuntControl, SwitchedShuntMode, TapChanger,
    TapChangerKind, TapChangerRegulationMode, TapChangerStep, TemporaryLimit, Terminal,
    TerminalReference, TopologyEndpoint, TopologyKind, TopologySwitch, Transformer3W, VoltageLevel,
    VoltageSourceConverter, Winding,
};
use crate::{Error, Result};

const FMT: &str = "CGMES";
/// CGMES has no system MVA base; every per-unit value lands on this one.
const SYSTEM_MVA: f64 = 100.0;

#[cfg(test)]
pub(crate) struct Parsed {
    pub(crate) network: BalancedNetwork,
    pub(crate) warnings: CgmesDiagnostics,
}

/// The per-file role, from the `md:FullModel` profile URIs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Profile {
    Model,
    /// Diagram layout / geography / dynamics: valid parts of a set, but
    /// nothing the balanced model consumes; counted, not merged.
    Presentation,
}

#[derive(Clone, Copy)]
enum TypedLiteral {
    FiniteNumber,
    SignedInteger,
    NonnegativeInteger,
    PositiveInteger,
    PositiveTerminalNumber,
    Boolean,
}

#[allow(clippy::too_many_lines)] // exhaustive list of CGMES literals consumed below
fn typed_literal(property: &str) -> Option<TypedLiteral> {
    let (owner, field) = property.split_once('.')?;
    let integer = match (owner, field) {
        ("ACDCTerminal" | "Terminal", "sequenceNumber") | ("TransformerEnd", "endNumber") => {
            Some(TypedLiteral::PositiveTerminalNumber)
        }
        ("NonlinearShuntCompensatorPoint", "sectionNumber") => Some(TypedLiteral::PositiveInteger),
        ("ACDCConverter", "numberOfValves")
        | ("ShuntCompensator", "maximumSections" | "normalSections") => {
            Some(TypedLiteral::NonnegativeInteger)
        }
        ("TapChanger", "highStep" | "lowStep" | "neutralStep" | "normalStep")
        | ("TapChangerTablePoint", "step") => Some(TypedLiteral::SignedInteger),
        _ => None,
    };
    if integer.is_some() {
        return integer;
    }
    let boolean = matches!(
        (owner, field),
        ("ACDCTerminal", "connected")
            | ("Equipment", "aggregate" | "inService")
            | ("EquivalentInjection", "regulationStatus")
            | ("IdentifiedObject", "isFictitious")
            | ("LoadResponseCharacteristic", "exponentModel")
            | ("OperationalLimitType", "isInfiniteDuration")
            | ("RegulatingCondEq", "controlEnabled")
            | ("RegulatingControl", "discrete" | "enabled")
            | ("SeriesCompensator", "varistorPresent")
            | ("SvStatus", "inService")
            | ("Switch", "normalOpen" | "open" | "retained")
            | ("TapChanger", "controlEnabled" | "ltcFlag")
    );
    if boolean {
        return Some(TypedLiteral::Boolean);
    }

    let number = match owner {
        "ACDCConverter" => matches!(
            field,
            "baseS"
                | "idc"
                | "idleLoss"
                | "maxP"
                | "maxUdc"
                | "minP"
                | "minUdc"
                | "numberOfValves"
                | "p"
                | "poleLossP"
                | "q"
                | "ratedUdc"
                | "resistiveLoss"
                | "switchingLoss"
                | "targetPpcc"
                | "targetUdc"
                | "uc"
                | "udc"
                | "valveU0"
        ),
        "ACDCTerminal" | "Terminal" => field == "sequenceNumber",
        "ACLineSegment" => matches!(field, "bch" | "gch" | "r" | "x"),
        "BaseFrequency" => field == "frequency",
        "BaseVoltage" => field == "nominalVoltage",
        "CsConverter" => matches!(
            field,
            "alpha"
                | "gamma"
                | "maxAlpha"
                | "maxGamma"
                | "minAlpha"
                | "minGamma"
                | "ratedIdc"
                | "targetAlpha"
                | "targetGamma"
                | "targetIdc"
        ),
        "CurveData" => matches!(field, "xvalue" | "y1value" | "y2value"),
        "DCConductingEquipment" => field == "ratedUdc",
        "DCGround" => matches!(field, "inductance" | "r"),
        "DCLineSegment" => matches!(
            field,
            "capacitance" | "inductance" | "length" | "resistance"
        ),
        "DCSeriesDevice" => matches!(field, "inductance" | "ratedUdc" | "resistance"),
        "EnergyConsumer" | "EquivalentInjection" => {
            matches!(field, "p" | "q")
        }
        "RotatingMachine" => matches!(field, "p" | "q" | "ratedS"),
        "ExternalNetworkInjection" => {
            matches!(field, "maxP" | "maxQ" | "minP" | "minQ" | "p" | "q")
        }
        "GeneratingUnit" => matches!(
            field,
            "initialP" | "maxOperatingP" | "minOperatingP" | "nominalP" | "normalPF"
        ),
        "LinearShuntCompensator" => matches!(field, "bPerSection" | "gPerSection"),
        "LoadResponseCharacteristic" => matches!(
            field,
            "pConstantCurrent"
                | "pConstantImpedance"
                | "pConstantPower"
                | "pVoltageExponent"
                | "qConstantCurrent"
                | "qConstantImpedance"
                | "qConstantPower"
                | "qVoltageExponent"
        ),
        "NonlinearShuntCompensatorPoint" => matches!(field, "b" | "g" | "sectionNumber"),
        "OperationalLimitType" => field == "acceptableDuration",
        "PhaseTapChanger" => matches!(field, "xStepMax" | "xStepMin"),
        "PhaseTapChangerAsymmetrical" => field == "windingConnectionAngle",
        "PhaseTapChangerLinear" => matches!(field, "stepPhaseShiftIncrement" | "xMax" | "xMin"),
        "PhaseTapChangerNonLinear" => matches!(field, "voltageStepIncrement" | "xMax" | "xMin"),
        "PhaseTapChangerSymmetrical" => field == "stepPhaseShiftIncrement",
        "PhaseTapChangerTablePoint" => field == "angle",
        "PowerTransformerEnd" => matches!(field, "b" | "g" | "r" | "ratedU" | "x"),
        "RatioTapChanger" => field == "stepVoltageIncrement",
        "RegulatingControl" => matches!(field, "targetDeadband" | "targetValue"),
        "SeriesCompensator" => matches!(
            field,
            "r" | "r0" | "varistorRatedCurrent" | "varistorVoltageThreshold" | "x" | "x0"
        ),
        "ShuntCompensator" => matches!(field, "maximumSections" | "normalSections" | "sections"),
        "StaticVarCompensator" => matches!(
            field,
            "capacitiveRating" | "inductiveRating" | "p" | "q" | "voltageSetPoint"
        ),
        "SvPowerFlow" => matches!(field, "p" | "q"),
        "SvShuntCompensatorSections" => field == "sections",
        "SvTapStep" => field == "position",
        "SvVoltage" => matches!(field, "angle" | "v"),
        "Switch" => field == "ratedCurrent",
        "SynchronousMachine" => matches!(field, "maxQ" | "minQ" | "referencePriority"),
        "TapChanger" => matches!(
            field,
            "highStep" | "lowStep" | "neutralStep" | "normalStep" | "step"
        ),
        "TapChangerTablePoint" => matches!(field, "b" | "g" | "r" | "ratio" | "step" | "x"),
        "TransformerEnd" => field == "endNumber",
        "VoltageLevel" => matches!(field, "highVoltageLimit" | "lowVoltageLimit"),
        "VsConverter" => matches!(
            field,
            "delta"
                | "droop"
                | "droopCompensation"
                | "maxModulationIndex"
                | "maxValveCurrent"
                | "qShare"
                | "targetQpcc"
                | "targetUpcc"
                | "uf"
                | "uv"
        ),
        // CurrentLimit, ApparentPowerLimit, ActivePowerLimit, and
        // VoltageLimit share the OperationalLimit value attributes.
        class if class.ends_with("Limit") => matches!(field, "normalValue" | "value"),
        _ => false,
    };
    number.then_some(TypedLiteral::FiniteNumber)
}

fn validate_typed_literals(class: &str, id: &str, props: &[(String, PropValue)]) -> Result<()> {
    for (property, value) in props {
        let Some(kind) = typed_literal(property) else {
            continue;
        };
        let PropValue::Text(text) = value else {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "{class} `{id}` property {property} is an RDF resource reference, not a typed literal"
                ),
            });
        };
        let valid = match kind {
            TypedLiteral::FiniteNumber => text.trim().parse::<f64>().is_ok_and(f64::is_finite),
            TypedLiteral::SignedInteger => text.trim().parse::<f64>().is_ok_and(|value| {
                value.is_finite()
                    && value.fract() == 0.0
                    && value >= f64::from(i32::MIN)
                    && value <= f64::from(i32::MAX)
            }),
            TypedLiteral::NonnegativeInteger => text.trim().parse::<f64>().is_ok_and(|value| {
                value.is_finite()
                    && value.fract() == 0.0
                    && value >= 0.0
                    && value <= f64::from(u32::MAX)
            }),
            TypedLiteral::PositiveInteger => text.trim().parse::<f64>().is_ok_and(|value| {
                value.is_finite()
                    && value.fract() == 0.0
                    && (1.0..=f64::from(u32::MAX)).contains(&value)
            }),
            TypedLiteral::PositiveTerminalNumber => text.trim().parse::<f64>().is_ok_and(|value| {
                value.is_finite()
                    && value.fract() == 0.0
                    && (1.0..=f64::from(u8::MAX)).contains(&value)
            }),
            TypedLiteral::Boolean => matches!(
                text.trim(),
                "true" | "TRUE" | "True" | "1" | "false" | "FALSE" | "False" | "0"
            ),
        };
        if !valid {
            let expected = match kind {
                TypedLiteral::FiniteNumber => "a finite number",
                TypedLiteral::SignedInteger => "an integer from -2147483648 through 2147483647",
                TypedLiteral::NonnegativeInteger => "an integer from 0 through 4294967295",
                TypedLiteral::PositiveInteger => "an integer from 1 through 4294967295",
                TypedLiteral::PositiveTerminalNumber => "an integer from 1 through 255",
                TypedLiteral::Boolean => "true, false, 1, or 0",
            };
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "{class} `{id}` property {property} has invalid value `{text}`; expected {expected}"
                ),
            });
        }
    }
    Ok(())
}

fn classify(header: Option<&ModelHeader>) -> Profile {
    let Some(header) = header else {
        return Profile::Model;
    };
    let presentation = header.profiles.iter().all(|p| {
        p.contains("DiagramLayout") || p.contains("GeographicalLocation") || p.contains("Dynamics")
    });
    if presentation && !header.profiles.is_empty() {
        Profile::Presentation
    } else {
        Profile::Model
    }
}

fn warn_full_model_fields(
    document_name: &str,
    header: Option<&ModelHeader>,
    warnings: &mut CgmesDiagnostics,
) {
    let Some(header) = header else {
        return;
    };
    // A header this writer synthesized restates nothing: the identity, the
    // authority set, the creation time, the version, and the dependency
    // references are all values fresh emission assigns, so reporting them as
    // dropped would make every conversion into CGMES declare a loss the
    // source never stated.
    if header.modeling_authority_set.as_deref() == Some(super::POWERIO_MODELING_AUTHORITY_SET) {
        warn_full_model_extra_properties(document_name, header, warnings);
        return;
    }
    if let Some(identity) = header.identity.as_deref() {
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "FullModel RDF identity `{identity}` in `{document_name}` is not retained in the electrical model; fresh CGMES emission assigns a deterministic FullModel identity"
            ),
        );
    }
    if let Some(authority) = header.modeling_authority_set.as_deref() {
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "FullModel property `Model.modelingAuthoritySet` in `{document_name}` is `{authority}`; it identifies the authority of the records this document defines during boundary and state variable checks and is not retained in the electrical model, so fresh CGMES emission states PowerIO's own modeling authority"
            ),
        );
    }
    if let Some(created) = header.created.as_deref() {
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "FullModel property `Model.created` in `{document_name}` is `{created}` and is not retained; `Model.scenarioTime`, when present, remains the network case date"
            ),
        );
    }
    if let Some(version) = header.version.as_deref() {
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "FullModel property `Model.version` in `{document_name}` is `{version}` and is not retained; fresh CGMES emission writes its own document version"
            ),
        );
    }
    if !header.dependent_on.is_empty() {
        let samples = header
            .dependent_on
            .iter()
            .take(5)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("`, `");
        let remainder = if header.dependent_on.len() > 5 {
            format!(" and {} more", header.dependent_on.len() - 5)
        } else {
            String::new()
        };
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "FullModel in `{document_name}` has {} `Model.DependentOn` reference(s) [`{samples}`]{remainder}; the merged electrical model does not retain profile document dependencies and fresh emission rebuilds them",
                header.dependent_on.len()
            ),
        );
    }
    warn_full_model_extra_properties(document_name, header, warnings);
}

/// Report the FullModel properties beyond the mapped set, whoever wrote them:
/// a property this reader does not map is stated by the document either way.
fn warn_full_model_extra_properties(
    document_name: &str,
    header: &ModelHeader,
    warnings: &mut CgmesDiagnostics,
) {
    if !header.unmapped_properties.is_empty() {
        let mut grouped: BTreeMap<&str, usize> = BTreeMap::new();
        for (property, _) in &header.unmapped_properties {
            *grouped.entry(property).or_default() += 1;
        }
        let fields = grouped
            .iter()
            .map(|(property, count)| format!("`{property}` ({count})"))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "FullModel in `{document_name}` has {} unmapped property value(s): {fields}; fresh CGMES emission omits them",
                header.unmapped_properties.len()
            ),
        );
    }
    if !header.nested_properties.is_empty() {
        let mut grouped: BTreeMap<&str, usize> = BTreeMap::new();
        for property in &header.nested_properties {
            *grouped.entry(property).or_default() += 1;
        }
        let fields = grouped
            .iter()
            .map(|(property, count)| format!("`{property}` ({count})"))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "FullModel in `{document_name}` has {} property value(s) encoded as nested RDF/XML: {fields}; a header property reaches the electrical model as one text value and a nested structure has no such spelling",
                header.nested_properties.len()
            ),
        );
    }
}

fn skipped_part_summary(document_name: &str, doc: &CimDocument) -> String {
    let profiles = doc
        .header
        .as_ref()
        .map(|header| header.profiles.join(", "))
        .unwrap_or_default();
    let mut classes: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for object in &doc.objects {
        classes
            .entry(object.class.as_str())
            .or_default()
            .push(object.id.as_str());
    }
    let classes = classes
        .into_iter()
        .map(|(class, ids)| {
            let samples = ids.iter().take(3).copied().collect::<Vec<_>>().join("`, `");
            let remainder = if ids.len() > 3 {
                format!(" and {} more", ids.len() - 3)
            } else {
                String::new()
            };
            format!("{class}: {} [`{samples}`]{remainder}", ids.len())
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "`{document_name}` profiles [{profiles}] contain {} presentation/dynamics object(s) that are not mapped ({classes})",
        doc.objects.len()
    )
}

/// One object merged across the profile files.
struct Merged {
    id: String,
    class: String,
    defined: bool,
    modeling_authority_set: Option<String>,
    props: Vec<(String, PropValue)>,
}

/// The merged object store plus the id and class indexes the mapping reads.
#[derive(Default)]
struct Store {
    objects: Vec<Merged>,
    by_id: HashMap<String, usize>,
    /// Properties successfully read by the source neutral mapping. This lets
    /// the final diagnostic pass distinguish a mapped class from fields on
    /// that class which the mapping did not consume.
    read_props: RefCell<BTreeSet<(usize, usize)>>,
    /// Whether a document declared this writer's modeling authority. The
    /// declaration permits suppressing only the exact container, island, and
    /// subordinate values fresh emission synthesizes; it does not make other
    /// classes or properties safe to ignore.
    own_output: bool,
    /// Whether any document declared a modeling authority other than this
    /// writer's own, which makes the set a third party document set.
    foreign_output: bool,
}

/// CIM100 SSH serializes the inherited `Equipment.inService` property in an
/// `Equipment rdf:about` element, even when EQ defines the same object as a
/// concrete subclass such as `BusbarSection`. It is an extension of that
/// object, not a second object with a conflicting class.
fn is_profile_extension_class(class: &str) -> bool {
    class == "Equipment"
}

impl Store {
    fn merge(&mut self, doc: CimDocument) -> Result<()> {
        let modeling_authority_set = doc
            .header
            .as_ref()
            .and_then(|header| header.modeling_authority_set.clone());
        match modeling_authority_set.as_deref() {
            Some(super::POWERIO_MODELING_AUTHORITY_SET) => self.own_output = true,
            Some(_) => self.foreign_output = true,
            None => {}
        }
        for CimObject {
            class,
            id,
            definition,
            props,
        } in doc.objects
        {
            if id.is_empty() {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!("{class} object has no rdf:ID or rdf:about identifier"),
                });
            }
            validate_typed_literals(&class, &id, &props)?;
            if let Some(&at) = self.by_id.get(&id) {
                if self.objects[at].class != class {
                    if !definition && is_profile_extension_class(&class) {
                        merge_properties(&id, &mut self.objects[at].props, props)?;
                        continue;
                    }
                    if !self.objects[at].defined
                        && is_profile_extension_class(&self.objects[at].class)
                    {
                        self.objects[at].class = class;
                    } else {
                        return Err(Error::FormatRead {
                            format: FMT,
                            message: format!(
                                "RDF identifier `{id}` is used for both {} and {class}",
                                self.objects[at].class
                            ),
                        });
                    }
                }
                if definition && self.objects[at].defined {
                    return Err(Error::FormatRead {
                        format: FMT,
                        message: format!("RDF identifier `{id}` is defined more than once"),
                    });
                }
                if definition {
                    self.objects[at]
                        .modeling_authority_set
                        .clone_from(&modeling_authority_set);
                }
                self.objects[at].defined |= definition;
                merge_properties(&id, &mut self.objects[at].props, props)?;
            } else {
                self.by_id.insert(id.clone(), self.objects.len());
                let mut merged_props = Vec::new();
                merge_properties(&id, &mut merged_props, props)?;
                self.objects.push(Merged {
                    id,
                    class,
                    defined: definition,
                    modeling_authority_set: if definition {
                        modeling_authority_set.clone()
                    } else {
                        None
                    },
                    props: merged_props,
                });
            }
        }
        Ok(())
    }

    fn class_of(&self, id: &str) -> Option<&str> {
        self.by_id
            .get(id)
            .map(|&at| self.objects[at].class.as_str())
    }

    fn modeling_authority_set(&self, id: &str) -> Option<&str> {
        self.by_id
            .get(id)
            .and_then(|&at| self.objects[at].modeling_authority_set.as_deref())
    }

    fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    /// Ids of every object of `class`, in first-definition order.
    fn of_class<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a str> {
        self.objects
            .iter()
            .filter(move |o| o.class == class)
            .map(|o| o.id.as_str())
    }

    fn raw_prop(&self, id: &str, key: &str) -> Option<(usize, usize, &PropValue)> {
        let &at = self.by_id.get(id)?;
        self.objects[at]
            .props
            .iter()
            .enumerate()
            .find(|(_, (property, _))| property == key)
            .map(|(property_at, (_, value))| (at, property_at, value))
    }

    fn mark_read(&self, object_at: usize, property_at: usize) {
        self.read_props
            .borrow_mut()
            .insert((object_at, property_at));
    }

    fn text(&self, id: &str, key: &str) -> Option<&str> {
        let (object_at, property_at, value) = self.raw_prop(id, key)?;
        let PropValue::Text(value) = value else {
            return None;
        };
        self.mark_read(object_at, property_at);
        Some(value)
    }

    fn f(&self, id: &str, key: &str) -> Option<f64> {
        Some(
            self.text(id, key)?
                .trim()
                .parse()
                .expect("CGMES numeric properties are validated while profiles are merged"),
        )
    }

    fn boolean(&self, id: &str, key: &str) -> Option<bool> {
        match self.text(id, key)?.trim() {
            "true" | "TRUE" | "True" | "1" => Some(true),
            "false" | "FALSE" | "False" | "0" => Some(false),
            _ => unreachable!("CGMES boolean properties are validated while profiles are merged"),
        }
    }

    /// A reference property's target id.
    fn refv(&self, id: &str, key: &str) -> Option<&str> {
        let (object_at, property_at, value) = self.raw_prop(id, key)?;
        let PropValue::Ref(target) = value else {
            return None;
        };
        self.mark_read(object_at, property_at);
        Some(target)
    }

    /// The `value` half of an `EnumClass.value` reference.
    fn enum_value(&self, id: &str, key: &str) -> Option<&str> {
        self.refv(id, key)?.rsplit('.').next()
    }

    fn name(&self, id: &str) -> String {
        self.text(id, "IdentifiedObject.name")
            .map_or_else(|| id.to_string(), str::to_string)
    }
}

fn merge_properties(
    object: &str,
    merged: &mut Vec<(String, PropValue)>,
    incoming: Vec<(String, PropValue)>,
) -> Result<()> {
    for (property, value) in incoming {
        if let Some((_, existing)) = merged.iter().find(|(name, _)| name == &property) {
            if existing != &value && repeatable_property(&property) {
                merged.push((property, value));
            } else if existing != &value {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "RDF object `{object}` assigns conflicting `{property}` values: {} and {}",
                        describe_property_value(existing),
                        describe_property_value(&value),
                    ),
                });
            }
        } else {
            merged.push((property, value));
        }
    }
    Ok(())
}

fn repeatable_property(property: &str) -> bool {
    matches!(
        property,
        "TopologicalIsland.TopologicalNodes"
            | "DCTopologicalIsland.DCTopologicalNodes"
            | "ConnectivityNode.Terminals"
            | "TopologicalNode.ConnectivityNodes"
    )
}

fn describe_property_value(value: &PropValue) -> String {
    match value {
        PropValue::Text(value) => format!("text `{value}`"),
        PropValue::Ref(value) => format!("reference `{value}`"),
    }
}

/// Where the balanced buses come from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BusSource {
    /// One bus per `TopologicalNode` record.
    TopologicalNodes,
    /// One bus per connected component of `ConnectivityNode` records joined
    /// by closed, in service switches.
    Calculated,
}

/// Terminal wiring: equipment → its terminals (sequence order), terminal →
/// bus node (the topological node, or the connectivity node when buses are
/// calculated), and the terminal SSH connection value.
struct Wiring {
    of_equipment: HashMap<String, Vec<String>>,
    node_of: HashMap<String, String>,
    connected: HashMap<String, bool>,
}

impl Wiring {
    fn build(store: &Store, topology: BusSource) -> Wiring {
        let mut of_equipment: HashMap<String, Vec<(f64, String)>> = HashMap::new();
        let mut node_of = HashMap::new();
        let mut connected = HashMap::new();
        for id in store.of_class("Terminal") {
            if let Some(eq) = store.refv(id, "Terminal.ConductingEquipment") {
                let seq = store
                    .f(id, "ACDCTerminal.sequenceNumber")
                    .or_else(|| store.f(id, "Terminal.sequenceNumber"))
                    .unwrap_or(1.0);
                of_equipment
                    .entry(eq.to_string())
                    .or_default()
                    .push((seq, id.to_string()));
            }
            // 2.4.15 TP links the terminal itself; 3.0 links through the
            // connectivity node. Either way the terminal lands on a TN. A
            // calculated topology keys buses by the connectivity node.
            let node = match topology {
                BusSource::TopologicalNodes => terminal_topological_node(store, id),
                BusSource::Calculated => store.refv(id, "Terminal.ConnectivityNode"),
            };
            if let Some(node) = node {
                node_of.insert(id.to_string(), node.to_string());
            }
            if let Some(is_connected) = store.boolean(id, "ACDCTerminal.connected") {
                connected.insert(id.to_string(), is_connected);
            }
        }
        let of_equipment = of_equipment
            .into_iter()
            .map(|(eq, mut terms)| {
                terms.sort_by(|a, b| a.0.total_cmp(&b.0));
                (eq, terms.into_iter().map(|(_, t)| t).collect())
            })
            .collect();
        Wiring {
            of_equipment,
            node_of,
            connected,
        }
    }

    fn terminals(&self, equipment: &str) -> &[String] {
        self.of_equipment.get(equipment).map_or(&[], Vec::as_slice)
    }

    fn node(&self, terminal: &str) -> Option<&str> {
        self.node_of.get(terminal).map(String::as_str)
    }

    /// In service as far as the terminals say: every terminal connected
    /// (missing SSH data reads as connected).
    fn energized(&self, equipment: &str) -> bool {
        self.terminals(equipment)
            .iter()
            .all(|t| self.connected.get(t).copied().unwrap_or(true))
    }
}

/// One calculated bus: the connectivity nodes it joins, in definition order,
/// and the voltage level or other connectivity container they resolve to.
struct CalculatedGroup {
    bus: BusId,
    container: String,
    nodes: Vec<String>,
}

/// Everything the element builders share.
struct Mapper<'a> {
    store: &'a Store,
    topology: BusSource,
    wiring: Wiring,
    /// Bus node identity (topological node, or connectivity node when the
    /// topology is calculated) → balanced bus.
    bus_of_node: HashMap<String, BusId>,
    /// The calculated buses, empty when TopologicalNode records supplied them.
    calculated: Vec<CalculatedGroup>,
    kv_of: HashMap<BusId, f64>,
    /// Terminal → solved SvPowerFlow (p, q), the SSH fallback.
    sv_flow: HashMap<String, (f64, f64)>,
    /// Conducting equipment → solved service status. SV takes precedence
    /// over SSH Equipment.inService when both profiles carry a value.
    sv_status: HashMap<String, bool>,
    warnings: &'a mut CgmesDiagnostics,
}

impl Mapper<'_> {
    fn bus_of_equipment_terminal(&mut self, equipment: &str, index: usize) -> Option<BusId> {
        let terminal = self.wiring.terminals(equipment).get(index)?.clone();
        let node = self.wiring.node(&terminal)?.to_string();
        self.bus_of_node.get(node.as_str()).copied()
    }

    /// The balanced bus a topological node maps to; `None` when the topology
    /// was calculated, because the set then defines no topological nodes.
    fn bus_of_topological_node(&self, topological_node: &str) -> Option<BusId> {
        match self.topology {
            BusSource::TopologicalNodes => self.bus_of_node.get(topological_node).copied(),
            BusSource::Calculated => None,
        }
    }

    fn kv(&self, bus: BusId) -> f64 {
        self.kv_of[&bus]
    }

    /// SSH value with SvPowerFlow fallback (vendor sets without SSH).
    fn power(&mut self, equipment: &str, key: &str) -> (f64, f64) {
        let store = self.store;
        let ssh_p = store.f(equipment, &format!("{key}.p"));
        let ssh_q = store.f(equipment, &format!("{key}.q"));
        let solved = self
            .wiring
            .terminals(equipment)
            .iter()
            .find_map(|terminal| self.sv_flow.get(terminal).copied());
        match (ssh_p, ssh_q, solved) {
            (Some(p), Some(q), _) => (p, q),
            (None, None, Some((p, q))) => {
                self.warnings.push_as(&codes::READ_CGMES_VALUE_APPROXIMATED, format!(
                    "{key} `{equipment}` has no SSH p or q assignment; PowerIO used its complete SvPowerFlow terminal result p={p} MW and q={q} MVAr"
                ));
                (p, q)
            }
            (Some(p), None, Some((_, q))) => {
                self.warnings.push_as(&codes::READ_CGMES_VALUE_APPROXIMATED, format!(
                    "{key} `{equipment}` has SSH p={p} MW but no SSH q assignment; PowerIO kept p and used q={q} MVAr from its SvPowerFlow terminal result"
                ));
                (p, q)
            }
            (None, Some(q), Some((p, _))) => {
                self.warnings.push_as(&codes::READ_CGMES_VALUE_APPROXIMATED, format!(
                    "{key} `{equipment}` has SSH q={q} MVAr but no SSH p assignment; PowerIO kept q and used p={p} MW from its SvPowerFlow terminal result"
                ));
                (p, q)
            }
            (Some(p), None, None) => {
                self.warnings.push_as(&codes::READ_CGMES_VALUE_DEFAULTED, format!(
                    "{key} `{equipment}` has SSH p={p} MW but no q assignment or complete SvPowerFlow terminal result; q was defaulted to 0 MVAr"
                ));
                (p, 0.0)
            }
            (None, Some(q), None) => {
                self.warnings.push_as(&codes::READ_CGMES_VALUE_DEFAULTED, format!(
                    "{key} `{equipment}` has SSH q={q} MVAr but no p assignment or complete SvPowerFlow terminal result; p was defaulted to 0 MW"
                ));
                (0.0, q)
            }
            (None, None, None) => {
                self.warnings.push_as(&codes::READ_CGMES_VALUE_DEFAULTED, format!(
                    "{key} `{equipment}` has neither SSH p/q assignments nor a complete SvPowerFlow terminal result; p and q were defaulted to zero"
                ));
                (0.0, 0.0)
            }
        }
    }

    fn in_service(&mut self, equipment: &str) -> bool {
        let sv = self.sv_status.get(equipment).copied();
        let ssh = self.store.boolean(equipment, "Equipment.inService");
        match (sv, ssh) {
            (Some(sv), Some(ssh)) => {
                if sv != ssh {
                    self.warnings.push_as(
                        &codes::READ_CGMES_VALUE_APPROXIMATED,
                        format!(
                            "equipment `{equipment}` has SvStatus.inService={sv} but SSH Equipment.inService={ssh}; PowerIO used the solved SvStatus value"
                        ),
                    );
                }
                sv
            }
            (Some(sv), None) => sv,
            (None, Some(ssh)) => ssh,
            (None, None) => self.wiring.energized(equipment),
        }
    }
}

/// Read already acquired CGMES XML profile documents as one case.
///
/// Diagnostics collected before a failure are dropped with it; callers that
/// must keep them use [`read_cgmes_documents_into`].
#[cfg(test)]
pub(crate) fn read_cgmes_documents(
    documents: Vec<(String, String)>,
    name_hint: Option<&str>,
) -> Result<Parsed> {
    let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
    let network = read_cgmes_documents_into(documents, name_hint, &mut warnings)?;
    Ok(Parsed { network, warnings })
}

/// Read already acquired CGMES XML profile documents as one case, appending
/// every diagnostic to `warnings`, including the coded refusal that precedes
/// an error.
#[allow(clippy::too_many_lines)] // profile classification and merge share one ordered pass
pub(crate) fn read_cgmes_documents_into(
    documents: Vec<(String, String)>,
    name_hint: Option<&str>,
    warnings: &mut CgmesDiagnostics,
) -> Result<BalancedNetwork> {
    let mut store = Store::default();
    let mut versions: Vec<CgmesVersion> = Vec::new();
    let mut description: Option<String> = None;
    let mut scenario_time: Option<String> = None;
    let mut skipped: Vec<String> = Vec::new();
    let mut has_eq = false;
    let mut has_tp = false;
    let mut model_profiles: Vec<Vec<String>> = Vec::new();

    for (name, text) in documents {
        let doc = parse_cimxml(&text)?;
        for ns in &doc.cim_namespaces {
            if let Some(version) = CgmesVersion::from_namespace(ns) {
                if !versions.contains(&version) {
                    versions.push(version);
                }
            }
        }
        warn_full_model_fields(&name, doc.header.as_ref(), warnings);
        if let Some(header) = doc.header.as_ref() {
            if let Some(value) = header.scenario_time.as_ref() {
                if scenario_time
                    .as_ref()
                    .is_some_and(|current| current != value)
                {
                    warnings.push_as(
                        &codes::READ_CGMES_VALUE_APPROXIMATED,
                        format!(
                            "FullModel `Model.scenarioTime` in `{name}` is `{value}`, which conflicts with the retained case date `{}`; PowerIO keeps the first declared scenario time",
                            scenario_time.as_deref().unwrap_or_default()
                        ),
                    );
                } else {
                    scenario_time = Some(value.clone());
                }
            }
            if let Some(value) = header.description.as_ref() {
                if description.as_ref().is_some_and(|current| current != value) {
                    warnings.push_as(
                        &codes::READ_CGMES_VALUE_APPROXIMATED,
                        format!(
                            "FullModel `Model.description` in `{name}` is `{value}`, which conflicts with the retained network name `{}`; PowerIO keeps the first declared description",
                            description.as_deref().unwrap_or_default()
                        ),
                    );
                } else {
                    description = Some(value.clone());
                }
            }
        }
        if classify(doc.header.as_ref()) == Profile::Presentation {
            skipped.push(skipped_part_summary(&name, &doc));
            continue;
        }
        if let Some(header) = doc.header.as_ref() {
            has_eq |= header
                .profiles
                .iter()
                .any(|profile| is_equipment_core(profile));
            has_tp |= header.profiles.iter().any(|profile| {
                profile.contains("/Topology/") || profile.contains("/CIM/Topology-")
            });
            model_profiles.push(header.profiles.clone());
        }
        store.merge(doc)?;
    }

    let version = match versions.as_slice() {
        [] => {
            return Err(Error::FormatRead {
                format: FMT,
                message: "no file declares a CIM16/CIM100 namespace; not a CGMES set".into(),
            });
        }
        [one] => *one,
        many => {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "one profile set declares more than one CGMES release: {}",
                    many.iter()
                        .map(|version| version.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    };
    let topology = if store.of_class("TopologicalNode").next().is_some() {
        BusSource::TopologicalNodes
    } else {
        BusSource::Calculated
    };
    validate_critical_references(&store, topology)?;
    if !skipped.is_empty() {
        for summary in skipped {
            warnings.push(summary);
        }
    }
    if !has_eq {
        return Err(Error::FormatRead {
            format: FMT,
            message: "CGMES profile set is missing required EQ profile data".into(),
        });
    }
    if topology == BusSource::Calculated {
        check_calculable_connectivity(&store, &model_profiles, has_tp, warnings)?;
    }

    build(
        &store,
        version,
        topology,
        description,
        scenario_time,
        name_hint,
        warnings,
    )
}

fn is_equipment_core(profile: &str) -> bool {
    profile.contains("/EquipmentCore/") || profile.contains("/CIM/CoreEquipment")
}

fn is_equipment_operation(profile: &str) -> bool {
    profile.contains("/EquipmentOperation/") || profile.contains("/CIM/Operation")
}

/// PowSybl's CGMES 3 equipment test: the profile URI starts with the
/// CoreEquipment-EU namespace and its version is 3.0 or later.
fn is_cgmes3_equipment_core(profile: &str) -> bool {
    const PREFIX: &str = "http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/";
    const FIRST: &str = "http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0";
    profile.starts_with(PREFIX) && profile >= FIRST
}

/// Whether the declared profile URIs describe a node breaker set, following
/// PowSybl's `CgmesModelTripleStore.computeIsNodeBreaker`: every CGMES 3
/// equipment document is node breaker once ConnectivityNodes exist; a CGMES
/// 2.4.15 set is node breaker only when each document declaring EquipmentCore
/// also declares EquipmentOperation, and each document declaring
/// EquipmentBoundary also declares EquipmentBoundaryOperation.
fn declares_node_breaker(model_profiles: &[Vec<String>], store: &Store) -> bool {
    let has_connectivity_nodes = store.of_class("ConnectivityNode").next().is_some();
    let all_cgmes3 = model_profiles
        .iter()
        .flatten()
        .all(|profile| !is_equipment_core(profile) || is_cgmes3_equipment_core(profile));
    if all_cgmes3 && has_connectivity_nodes {
        return true;
    }
    model_profiles.iter().all(|profiles| {
        let core = profiles.iter().any(|profile| is_equipment_core(profile));
        let operation = profiles
            .iter()
            .any(|profile| is_equipment_operation(profile));
        let bd = profiles
            .iter()
            .any(|profile| profile.contains("/EquipmentBoundary/"));
        let bd_operation = profiles
            .iter()
            .any(|profile| profile.contains("/EquipmentBoundaryOperation/"));
        (!core || operation) && (!bd || bd_operation)
    })
}

/// Refuse a set without TopologicalNode data unless it declares node breaker
/// equipment whose terminals reference ConnectivityNodes; the refusal names
/// the missing data.
fn check_calculable_connectivity(
    store: &Store,
    model_profiles: &[Vec<String>],
    has_tp: bool,
    warnings: &mut CgmesDiagnostics,
) -> Result<()> {
    let topology_state = if has_tp {
        "the TP profile data defines no TopologicalNode records"
    } else {
        "the set declares no TP profile data"
    };
    let node_count = store.of_class("ConnectivityNode").count();
    let terminal_count = store.of_class("Terminal").count();
    let connected_terminal_count = store
        .of_class("Terminal")
        .filter(|terminal| store.refv(terminal, "Terminal.ConnectivityNode").is_some())
        .count();
    let missing = if !declares_node_breaker(model_profiles, store) {
        Some(format!(
            "the declared profiles describe bus branch equipment (no EquipmentOperation or CGMES 3 CoreEquipment profile with ConnectivityNodes), so nothing states which terminals share a bus; the set has {node_count} ConnectivityNode record(s)"
        ))
    } else if node_count == 0 {
        Some("the node breaker profiles define no ConnectivityNode records".to_string())
    } else if connected_terminal_count == 0 {
        Some(format!(
            "none of the {terminal_count} Terminal record(s) references a ConnectivityNode"
        ))
    } else {
        None
    };
    if let Some(missing) = missing {
        let message = format!(
            "{topology_state} and buses cannot be calculated: {missing}; a TP profile, or an EQ profile with ConnectivityNode connectivity and switch positions, is required"
        );
        warnings.push_as(
            &codes::READ_CGMES_CONNECTIVITY_INSUFFICIENT,
            message.clone(),
        );
        return Err(Error::FormatRead {
            format: FMT,
            message,
        });
    }
    Ok(())
}

fn validate_critical_references(store: &Store, topology: BusSource) -> Result<()> {
    const REQUIRED: [&str; 33] = [
        "Terminal.ConductingEquipment",
        "Terminal.TopologicalNode",
        "Terminal.ConnectivityNode",
        "ConnectivityNode.TopologicalNode",
        "TopologicalNode.BaseVoltage",
        "ConductingEquipment.BaseVoltage",
        "PowerTransformerEnd.PowerTransformer",
        "TransformerEnd.Terminal",
        "SvVoltage.TopologicalNode",
        "SvPowerFlow.Terminal",
        "RatioTapChanger.TransformerEnd",
        "PhaseTapChanger.TransformerEnd",
        "TapChanger.TapChangerControl",
        "RegulatingControl.Terminal",
        "OperationalLimitSet.Terminal",
        "OperationalLimitSet.Equipment",
        "OperationalLimit.OperationalLimitSet",
        "OperationalLimit.OperationalLimitType",
        "SvTapStep.TapChanger",
        "DCTerminal.DCConductingEquipment",
        "ACDCConverterDCTerminal.DCConductingEquipment",
        "DCBaseTerminal.DCNode",
        "DCBaseTerminal.DCTopologicalNode",
        "DCNode.DCTopologicalNode",
        "ACDCConverter.PccTerminal",
        "DCNode.DCEquipmentContainer",
        "DCTopologicalNode.DCEquipmentContainer",
        "DCConverterUnit.Substation",
        "Equipment.EquipmentContainer",
        "VsConverter.CapabilityCurve",
        "SynchronousMachine.InitialReactiveCapabilityCurve",
        "CurveData.Curve",
        "SvStatus.ConductingEquipment",
    ];
    for object in &store.objects {
        for (property, value) in &object.props {
            if !REQUIRED.contains(&property.as_str()) {
                continue;
            }
            let PropValue::Ref(target) = value else {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "{} `{}` property {property} is not an RDF resource reference",
                        object.class, object.id
                    ),
                });
            };
            // An SV profile kept without its TP profile observes topological
            // nodes the set never defines; those observations are counted
            // and left unmapped when the buses are calculated.
            if topology == BusSource::Calculated && property == "SvVoltage.TopologicalNode" {
                continue;
            }
            if !store.contains(target) {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "{} `{}` property {property} references missing `{target}`",
                        object.class, object.id
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Switch classes, all mapping to the neutral switch record.
const SWITCH_CLASSES: [&str; 8] = [
    "Breaker",
    "Disconnector",
    "LoadBreakSwitch",
    "Switch",
    "Fuse",
    "Jumper",
    "GroundDisconnector",
    "DisconnectingCircuitBreaker",
];

/// Load classes sharing the EnergyConsumer attribute set.
const LOAD_CLASSES: [&str; 4] = [
    "EnergyConsumer",
    "ConformLoad",
    "NonConformLoad",
    "StationSupply",
];

/// Classes the mapping consumes structurally (no "unmapped" warning).
const CONSUMED: &[&str] = &[
    "TopologicalNode",
    "ConnectivityNode",
    "Terminal",
    "BaseVoltage",
    "BaseFrequency",
    "SvVoltage",
    "SvPowerFlow",
    "SvStatus",
    "SvTapStep",
    "SvShuntCompensatorSections",
    "TopologicalIsland",
    "ACLineSegment",
    "SeriesCompensator",
    "PowerTransformer",
    "PowerTransformerEnd",
    "RatioTapChanger",
    "PhaseTapChangerLinear",
    "PhaseTapChangerNonLinear",
    "PhaseTapChangerSymmetrical",
    "PhaseTapChangerAsymmetrical",
    "PhaseTapChangerTabular",
    "RatioTapChangerTable",
    "RatioTapChangerTablePoint",
    "PhaseTapChangerTable",
    "PhaseTapChangerTablePoint",
    "SynchronousMachine",
    "GeneratingUnit",
    "ThermalGeneratingUnit",
    "HydroGeneratingUnit",
    "WindGeneratingUnit",
    "SolarGeneratingUnit",
    "NuclearGeneratingUnit",
    "ExternalNetworkInjection",
    "LinearShuntCompensator",
    "EquivalentInjection",
    "StaticVarCompensator",
    "DCConverterUnit",
    "DCNode",
    "DCTopologicalNode",
    "DCTerminal",
    "ACDCConverterDCTerminal",
    "DCGround",
    "DCBusbar",
    "DCLine",
    "DCLineSegment",
    "DCSeriesDevice",
    "DCSwitch",
    "DCBreaker",
    "DCDisconnector",
    "VsConverter",
    "CsConverter",
    "VsCapabilityCurve",
    "ReactiveCapabilityCurve",
    "CurveData",
    "Junction",
];

/// The balanced buses and the node and voltage indexes built with them.
struct BusTable {
    buses: Vec<Bus>,
    bus_of_node: HashMap<String, BusId>,
    kv_of: HashMap<BusId, f64>,
    calculated: Vec<CalculatedGroup>,
}

/// Buses from TopologicalNode records, ids in definition order.
fn buses_from_topological_nodes(store: &Store) -> Result<BusTable> {
    let mut buses = Vec::new();
    let mut bus_of_node = HashMap::new();
    let mut kv_of = HashMap::new();
    for (i, tn) in store.of_class("TopologicalNode").enumerate() {
        let id = BusId(i + 1);
        let base_voltage = store
            .refv(tn, "TopologicalNode.BaseVoltage")
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!(
                    "TopologicalNode `{tn}` has no TopologicalNode.BaseVoltage reference"
                ),
            })?;
        let kv = store
            .f(base_voltage, "BaseVoltage.nominalVoltage")
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!(
                    "TopologicalNode `{tn}` references BaseVoltage `{base_voltage}` without BaseVoltage.nominalVoltage"
                ),
            })?;
        if kv <= 0.0 {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "TopologicalNode `{tn}` references BaseVoltage `{base_voltage}` with nonpositive nominal voltage {kv} kV"
                ),
            });
        }
        let mut bus = Bus::new(id, BusType::Pq, kv);
        bus.name = Some(store.name(tn));
        bus.uid = Some(tn.to_string());
        buses.push(bus);
        bus_of_node.insert(tn.to_string(), id);
        kv_of.insert(id, kv);
    }
    Ok(BusTable {
        buses,
        bus_of_node,
        kv_of,
        calculated: Vec::new(),
    })
}

/// The switch position that decides whether a switch conducts: the SSH
/// `Switch.open` assignment, else the EQ `Switch.normalOpen` default, else
/// closed. PowSybl's `SwitchConversion.update` applies the same precedence.
fn switch_is_open(store: &Store, switch: &str) -> bool {
    store
        .boolean(switch, "Switch.open")
        .or_else(|| store.boolean(switch, "Switch.normalOpen"))
        .unwrap_or(false)
}

/// The service status a topology processor reads: the SV `SvStatus.inService`
/// observation, else the SSH `Equipment.inService` assignment, else in
/// service. CGMES defines `Equipment.inService` as availability for topology
/// processing, so a switch out of service never joins its nodes.
fn switch_in_service(store: &Store, switch: &str, sv_status: &HashMap<String, bool>) -> bool {
    sv_status
        .get(switch)
        .copied()
        .or_else(|| store.boolean(switch, "Equipment.inService"))
        .unwrap_or(true)
}

/// The nominal voltage of a calculated bus: its container's `VoltageLevel`
/// base voltage, else the base voltage stated by conducting equipment or a
/// transformer end attached to one of its nodes.
fn calculated_bus_kv(store: &Store, container: &str, nodes: &[String]) -> Option<f64> {
    if store.class_of(container) == Some("VoltageLevel")
        && let Some(kv) = store
            .refv(container, "VoltageLevel.BaseVoltage")
            .and_then(|base| store.f(base, "BaseVoltage.nominalVoltage"))
    {
        return Some(kv);
    }
    let node_set: BTreeSet<&str> = nodes.iter().map(String::as_str).collect();
    store
        .of_class("Terminal")
        .filter(|terminal| {
            store
                .refv(terminal, "Terminal.ConnectivityNode")
                .is_some_and(|node| node_set.contains(node))
        })
        .find_map(|terminal| {
            let equipment = store.refv(terminal, "Terminal.ConductingEquipment")?;
            let stated = store
                .refv(equipment, "ConductingEquipment.BaseVoltage")
                .and_then(|base| store.f(base, "BaseVoltage.nominalVoltage"));
            if stated.is_some() {
                return stated;
            }
            store.of_class("PowerTransformerEnd").find_map(|end| {
                (store.refv(end, "TransformerEnd.Terminal") == Some(terminal))
                    .then(|| {
                        store
                            .refv(end, "TransformerEnd.BaseVoltage")
                            .and_then(|base| store.f(base, "BaseVoltage.nominalVoltage"))
                            .or_else(|| store.f(end, "PowerTransformerEnd.ratedU"))
                    })
                    .flatten()
            })
        })
}

/// The deterministic identity of a calculated bus: UUIDv5 under PowerIO's
/// CGMES namespace over the sorted connectivity node identities it joins, so
/// the same nodes always yield the same bus mRID and never a source
/// TopologicalNode mRID.
fn calculated_bus_uid(nodes: &[String]) -> String {
    let namespace = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"https://powerio.dev/cgmes");
    let mut sorted: Vec<&str> = nodes.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let name = format!("calculated-bus:{}", sorted.join(","));
    uuid::Uuid::new_v5(&namespace, name.as_bytes()).to_string()
}

/// Buses as the connected components of ConnectivityNodes joined by closed,
/// in service switches; ids in first node definition order. PowSybl's node
/// breaker import builds the same graph (`NodeMapping`, `SwitchConversion`)
/// and lets IIDM compute the buses from it.
#[allow(clippy::too_many_lines)] // one pass joins nodes, names buses, and reports the result
fn calculate_buses(
    store: &Store,
    wiring: &Wiring,
    sv_status: &HashMap<String, bool>,
    warnings: &mut CgmesDiagnostics,
) -> Result<BusTable> {
    let nodes: Vec<&str> = store.of_class("ConnectivityNode").collect();
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (*node, position))
        .collect();
    let mut union = crate::format::union_find::UnionFind::new(nodes.len());
    let mut closed = 0usize;
    let mut open = 0usize;
    let mut out_of_service = 0usize;
    let mut unplaced = 0usize;
    for class in SWITCH_CLASSES {
        for switch in store.of_class(class) {
            let terminals = wiring.terminals(switch);
            let (Some(first), Some(second)) = (terminals.first(), terminals.get(1)) else {
                unplaced += 1;
                continue;
            };
            let ends = (
                store
                    .refv(first, "Terminal.ConnectivityNode")
                    .and_then(|node| index.get(node)),
                store
                    .refv(second, "Terminal.ConnectivityNode")
                    .and_then(|node| index.get(node)),
            );
            let (Some(first), Some(second)) = ends else {
                unplaced += 1;
                continue;
            };
            if switch_is_open(store, switch) {
                open += 1;
            } else if !switch_in_service(store, switch, sv_status) {
                out_of_service += 1;
            } else {
                closed += 1;
                union.union(*first, *second);
            }
        }
    }

    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of_root: HashMap<usize, usize> = HashMap::new();
    for position in 0..nodes.len() {
        let root = union.find(position);
        let group = *group_of_root.entry(root).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(position);
    }

    let busbar_names: HashMap<&str, String> = store
        .of_class("BusbarSection")
        .filter_map(|busbar| {
            let terminal = wiring.terminals(busbar).first()?;
            let node = store.refv(terminal, "Terminal.ConnectivityNode")?;
            Some((node, store.name(busbar)))
        })
        .collect();

    let referenced: BTreeSet<&str> = store
        .of_class("Terminal")
        .filter_map(|terminal| store.refv(terminal, "Terminal.ConnectivityNode"))
        .collect();
    let mut buses = Vec::new();
    let mut bus_of_node = HashMap::new();
    let mut kv_of = HashMap::new();
    let mut calculated = Vec::new();
    let mut split_levels = 0usize;
    let mut unreferenced: Vec<String> = Vec::new();
    for members in &groups {
        let member_ids: Vec<String> = members
            .iter()
            .map(|member| nodes[*member].to_string())
            .collect();
        let container = member_ids
            .iter()
            .find_map(|node| connectivity_node_container(store, node))
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!(
                    "ConnectivityNode `{}` has no ConnectivityNode.ConnectivityNodeContainer reference, so its calculated bus has no voltage level",
                    member_ids[0]
                ),
            })?
            .to_string();
        let Some(kv) = calculated_bus_kv(store, &container, &member_ids) else {
            if member_ids
                .iter()
                .all(|node| !referenced.contains(node.as_str()))
            {
                // An EQ_BD tie point no terminal of this set references (the
                // EQ_BD lists every interconnection) states no voltage and
                // connects nothing; PowSybl converts no node for it either.
                unreferenced.push(member_ids[0].clone());
                continue;
            }
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "calculated bus for ConnectivityNode `{}` has no nominal voltage: its container `{container}` is not a VoltageLevel with a BaseVoltage and no attached conducting equipment states a BaseVoltage",
                    member_ids[0]
                ),
            });
        };
        if kv <= 0.0 {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "calculated bus for ConnectivityNode `{}` has nonpositive nominal voltage {kv} kV",
                    member_ids[0]
                ),
            });
        }
        let id = BusId(buses.len() + 1);
        let (own, foreign): (Vec<String>, Vec<String>) = member_ids
            .iter()
            .cloned()
            .partition(|node| connectivity_node_container(store, node) == Some(container.as_str()));
        if !foreign.is_empty() {
            split_levels += 1;
            warnings.push_as(
                &codes::READ_CGMES_VALUE_APPROXIMATED,
                format!(
                    "calculated bus {id} joins ConnectivityNodes from more than one container through closed switches; it is placed in `{container}` and the {} node(s) from other containers (sample `{}`) map to it without a calculated bus record listing them",
                    foreign.len(),
                    foreign[0]
                ),
            );
        }
        let mut bus = Bus::new(id, BusType::Pq, kv);
        bus.name = Some(
            member_ids
                .iter()
                .find_map(|node| busbar_names.get(node.as_str()).cloned())
                .unwrap_or_else(|| store.name(&member_ids[0])),
        );
        bus.uid = Some(calculated_bus_uid(&member_ids));
        buses.push(bus);
        for node in &member_ids {
            bus_of_node.insert(node.clone(), id);
        }
        kv_of.insert(id, kv);
        calculated.push(CalculatedGroup {
            bus: id,
            container,
            nodes: own,
        });
    }

    let mut summary = format!(
        "no TopologicalNode data: {} calculated bus(es) joined {} ConnectivityNode(s) through {closed} closed switch(es); {open} open switch(es) separate nodes",
        buses.len(),
        nodes.len()
    );
    if out_of_service > 0 {
        let _ = write!(
            summary,
            "; {out_of_service} closed switch(es) out of service (SvStatus or Equipment.inService false) separate nodes"
        );
    }
    if unplaced > 0 {
        let _ = write!(
            summary,
            "; {unplaced} switch(es) without two ConnectivityNode terminals join nothing"
        );
    }
    if split_levels > 0 {
        let _ = write!(
            summary,
            "; {split_levels} bus(es) span more than one connectivity container"
        );
    }
    if !unreferenced.is_empty() {
        let _ = write!(
            summary,
            "; {} ConnectivityNode(s) outside any VoltageLevel that no terminal references (sample `{}`) got no bus",
            unreferenced.len(),
            unreferenced[0]
        );
    }
    summary.push_str(
        "; bus identities are UUIDv5 values derived from the joined ConnectivityNode mRIDs, not source TopologicalNode mRIDs",
    );
    warnings.push_as(&codes::READ_CGMES_TOPOLOGY_CALCULATED, summary);
    Ok(BusTable {
        buses,
        bus_of_node,
        kv_of,
        calculated,
    })
}

/// Conducting equipment → solved service status from SV `SvStatus` records.
fn read_sv_status(store: &Store) -> Result<HashMap<String, bool>> {
    let mut sv_status = HashMap::new();
    for status in store.of_class("SvStatus") {
        let equipment = store
            .refv(status, "SvStatus.ConductingEquipment")
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!(
                    "SvStatus {} has no SvStatus.ConductingEquipment reference",
                    store.name(status)
                ),
            })?;
        let in_service =
            store
                .boolean(status, "SvStatus.inService")
                .ok_or_else(|| Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "SvStatus {} has no boolean SvStatus.inService value",
                        store.name(status)
                    ),
                })?;
        if let Some(previous) = sv_status.insert(equipment.to_string(), in_service)
            && previous != in_service
        {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "conducting equipment {} has conflicting SvStatus.inService values",
                    store.name(equipment)
                ),
            });
        }
    }
    Ok(sv_status)
}

#[allow(clippy::too_many_lines)] // the element families map in one ordered pass
fn build(
    store: &Store,
    version: CgmesVersion,
    topology: BusSource,
    description: Option<String>,
    scenario_time: Option<String>,
    name_hint: Option<&str>,
    warnings: &mut CgmesDiagnostics,
) -> Result<BalancedNetwork> {
    let wiring = Wiring::build(store, topology);
    let sv_status = read_sv_status(store)?;

    let BusTable {
        mut buses,
        bus_of_node,
        kv_of,
        calculated,
    } = match topology {
        BusSource::TopologicalNodes => buses_from_topological_nodes(store)?,
        BusSource::Calculated => calculate_buses(store, &wiring, &sv_status, warnings)?,
    };
    if buses.is_empty() {
        return Err(Error::FormatRead {
            format: FMT,
            message: "the set defines no TopologicalNode and no ConnectivityNode records, so it has no buses".into(),
        });
    }
    let bus_of_topological_node = |topological_node: &str| match topology {
        BusSource::TopologicalNodes => bus_of_node.get(topological_node).copied(),
        BusSource::Calculated => None,
    };

    // SV voltage values onto the buses. Equal duplicate observations are
    // harmless; different observations for one topological node are ambiguous.
    let mut sv_voltage = HashMap::new();
    let mut unmapped_sv_voltages = 0usize;
    for sv in store.of_class("SvVoltage") {
        let Some(topological_node) = store.refv(sv, "SvVoltage.TopologicalNode") else {
            continue;
        };
        let observation = (store.f(sv, "SvVoltage.v"), store.f(sv, "SvVoltage.angle"));
        if let Some(previous) = sv_voltage.insert(topological_node.to_owned(), observation)
            && previous != observation
        {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "TopologicalNode `{topological_node}` has conflicting SvVoltage observations"
                ),
            });
        }
        let Some(bus) = bus_of_topological_node(topological_node) else {
            if topology == BusSource::Calculated {
                unmapped_sv_voltages += 1;
            }
            continue;
        };
        let bus = &mut buses[bus.0 - 1];
        if let Some(v) = observation.0 {
            bus.vm = v / bus.base_kv;
        }
        if let Some(angle) = observation.1 {
            bus.va = angle;
        }
    }
    if unmapped_sv_voltages > 0 {
        warnings.push_as(
            &codes::READ_CGMES_RECORD_UNMAPPED,
            format!(
                "{unmapped_sv_voltages} SvVoltage record(s) observe TopologicalNode identities the set does not define; the calculated buses keep their default voltage because no TP data ties those observations to ConnectivityNodes"
            ),
        );
    }
    for topological_node in store.of_class("TopologicalNode") {
        if let Some((sv_voltage, sv_authority, container_authority)) =
            sv_voltage_authority_mismatch(store, topological_node)
        {
            if let Some(equipment_authorities) =
                boundary_equipment_authorities(store, topological_node)
            {
                let voltage = store.f(sv_voltage, "SvVoltage.v");
                let angle = store.f(sv_voltage, "SvVoltage.angle");
                let observation = match (voltage, angle) {
                    (Some(voltage), Some(angle)) => {
                        format!("v={voltage} kV and angle={angle} degrees")
                    }
                    (Some(voltage), None) => format!("v={voltage} kV with no angle"),
                    (None, Some(angle)) => format!("no voltage with angle={angle} degrees"),
                    (None, None) => "no voltage or angle value".into(),
                };
                warnings.push(format!(
                    "SvVoltage `{sv_voltage}` for boundary TopologicalNode `{topological_node}` belongs to modelingAuthoritySet `{sv_authority}` and supplies {observation}; the node is shared by conducting equipment from modelingAuthoritySets [`{}`]. PowerIO maps the shared node to one boundary bus, so fresh CGMES omits this observation because it cannot reproduce distinct per-authority PowSybl boundary bus voltages",
                    equipment_authorities.join("`, `")
                ));
            } else {
                warnings.push(format!(
                    "SvVoltage `{sv_voltage}` for TopologicalNode `{topological_node}` belongs to modelingAuthoritySet `{sv_authority}`, while the node's ConnectivityNodeContainer belongs to `{container_authority}`; the raw v/angle observation is retained, but fresh single authority CGMES will omit it so PowSybl preserves the source network's unavailable bus voltage"
                ));
            }
        }
    }
    let mut sv_flow = HashMap::new();
    for sv in store.of_class("SvPowerFlow") {
        let Some(terminal) = store.refv(sv, "SvPowerFlow.Terminal") else {
            continue;
        };
        let active_power_mw = store.f(sv, "SvPowerFlow.p");
        let reactive_power_mvar = store.f(sv, "SvPowerFlow.q");
        if let Some((equipment, sv_authority, equipment_authority)) =
            sv_power_flow_authority_mismatch(store, sv)
            && (active_power_mw.is_some() || reactive_power_mvar.is_some())
        {
            warnings.push(format!(
                "SvPowerFlow `{sv}` for terminal `{terminal}` belongs to modelingAuthoritySet `{sv_authority}`, while conducting equipment `{equipment}` belongs to `{equipment_authority}`; its p={active_power_mw:?} MW and q={reactive_power_mvar:?} MVAr observations were not mapped to the other authority's equipment results"
            ));
            continue;
        }
        match (active_power_mw, reactive_power_mvar) {
            (Some(p), Some(q)) => {
                if let Some(previous) = sv_flow.insert(terminal.to_string(), (p, q))
                    && previous != (p, q)
                {
                    return Err(Error::FormatRead {
                        format: FMT,
                        message: format!(
                            "Terminal `{terminal}` has conflicting SvPowerFlow observations"
                        ),
                    });
                }
            }
            (None, None) => {}
            (Some(p), None) => warnings.push(format!(
                "SvPowerFlow `{sv}` for terminal `{terminal}` has active power {p} MW but no reactive power; CGMES requires both p and q, so the partial observation was not mapped"
            )),
            (None, Some(q)) => warnings.push(format!(
                "SvPowerFlow `{sv}` for terminal `{terminal}` has reactive power {q} MVAr but no active power; CGMES requires both p and q, so the partial observation was not mapped"
            )),
        }
    }
    let mut mapper = Mapper {
        store,
        topology,
        wiring,
        bus_of_node,
        calculated,
        kv_of,
        sv_flow,
        sv_status,
        warnings,
    };

    let mut loads = read_loads(&mut mapper);
    let (mut generators, ref_candidates, equipment_reactive_limits) = read_machines(&mut mapper)?;
    let shunts = read_shunts(&mut mapper)?;
    let static_var_compensators = read_static_var_compensators(&mut mapper)?;
    let (branches, transformers_3w) = read_branches(&mut mapper, version);
    let switches = read_switches(&mut mapper);
    read_equivalent_injections(&mut mapper, &mut loads, &mut generators);

    // Reference bus: the island's angle reference, else the best candidate
    // (lowest referencePriority, then an external injection, then the
    // largest machine).
    let mut reference: Option<BusId> = None;
    for island in store.of_class("TopologicalIsland") {
        if let Some(bus) = store
            .refv(island, "TopologicalIsland.AngleRefTopologicalNode")
            .and_then(|tn| mapper.bus_of_topological_node(tn))
        {
            reference = Some(bus);
            break;
        }
    }
    let reference = reference.or(ref_candidates);
    match reference {
        Some(id) => buses[id.0 - 1].kind = BusType::Ref,
        None => mapper.warnings.push(
            "no angle reference in the set (no TopologicalIsland, reference \
             priority, or external injection); matrix consumers will report \
             the missing slack",
        ),
    }
    for generator in &generators {
        let bus = &mut buses[generator.bus.0 - 1];
        if bus.kind == BusType::Pq {
            bus.kind = BusType::Pv;
        }
    }

    warn_regenerated_subordinate_identities(store, mapper.warnings);
    let detailed = build_detailed_connectivity(&mut mapper, version, equipment_reactive_limits)?;

    let base_frequency = store
        .of_class("BaseFrequency")
        .next()
        .and_then(|f| store.f(f, "BaseFrequency.frequency"))
        .unwrap_or_else(|| {
            mapper.warnings.push_as(
                &codes::READ_CGMES_VALUE_DEFAULTED,
                "no BaseFrequency record; assuming 50 Hz",
            );
            50.0
        });
    check_equipment_base_voltages(&mut mapper);
    warn_collapsed_base_voltage_identities(store, mapper.warnings);

    let name = description
        .filter(|d| !d.is_empty())
        .or_else(|| name_hint.map(str::to_string))
        .unwrap_or_else(|| "cgmes case".into());
    let mut net = BalancedNetwork::new(name, SYSTEM_MVA);
    net.case_metadata_mut().case_date = scenario_time;
    *net.base_frequency_mut() = base_frequency;
    *net.buses_mut() = buses;
    *net.loads_mut() = loads;
    *net.shunts_mut() = shunts;
    *net.static_var_compensators_mut() = static_var_compensators;
    *net.branches_mut() = branches;
    *net.transformers_3w_mut() = transformers_3w;
    *net.switches_mut() = switches;
    *net.generators_mut() = generators;
    *net.detailed_connectivity_mut() = Some(std::sync::Arc::new(detailed));
    *net.source_format_mut() = SourceFormat::Cgmes;
    net.assign_missing_component_ids();
    net.check_references(FMT)?;
    warn_unmapped(store, mapper.warnings);
    Ok(net)
}

fn component_id(kind: &str, id: &str) -> Result<ComponentId> {
    ComponentId::new(kind, id).map_err(|error| Error::FormatRead {
        format: FMT,
        message: error.to_string(),
    })
}

fn component_type(class: &str) -> &'static str {
    match class {
        "Substation" => "substation",
        "VoltageLevel" => "voltage_level",
        "TopologicalNode" => "bus",
        "ConnectivityNode" => "connectivity_node",
        "BusbarSection" => "busbar_section",
        "Junction" => "junction",
        "EnergyConsumer"
        | "ConformLoad"
        | "NonConformLoad"
        | "StationSupply"
        | "EquivalentInjection" => "load",
        "SynchronousMachine" | "ExternalNetworkInjection" => "generator",
        "LinearShuntCompensator" | "NonlinearShuntCompensator" => "shunt",
        "StaticVarCompensator" => "static_var_compensator",
        "Breaker"
        | "Disconnector"
        | "LoadBreakSwitch"
        | "Switch"
        | "Fuse"
        | "Jumper"
        | "GroundDisconnector"
        | "DisconnectingCircuitBreaker" => "switch",
        "ACLineSegment" | "SeriesCompensator" | "PowerTransformer" => "branch",
        "Line" => "line",
        "Terminal" => "terminal",
        "DCConverterUnit" => "dc_converter_unit",
        "DCNode" => "dc_node",
        "DCTopologicalNode" => "dc_topological_node",
        "DCGround" => "dc_ground",
        "DCBusbar" => "dc_busbar",
        "DCLine" => "dc_line_container",
        "DCLineSegment" => "dc_line",
        "DCSeriesDevice" => "dc_series_device",
        "DCSwitch" | "DCBreaker" | "DCDisconnector" => "dc_switch",
        "VsConverter" => "voltage_source_converter",
        "CsConverter" => "line_commutated_converter",
        "DCTerminal" | "ACDCConverterDCTerminal" => "dc_terminal",
        _ => "cgmes_object",
    }
}

/// The connectivity container a node or equipment container resolves to: a
/// `Bay` stands in for its `Bay.VoltageLevel`, because PowerIO's topology
/// records name voltage levels rather than bays; every other container (a
/// `VoltageLevel`, a `Line`, a `Substation`) is itself.
fn resolve_container<'a>(store: &'a Store, container: &'a str) -> &'a str {
    if store.class_of(container) == Some("Bay") {
        store
            .refv(container, "Bay.VoltageLevel")
            .unwrap_or(container)
    } else {
        container
    }
}

fn connectivity_node_container<'a>(store: &'a Store, node: &str) -> Option<&'a str> {
    store
        .refv(node, "ConnectivityNode.ConnectivityNodeContainer")
        .map(|container| resolve_container(store, container))
}

fn topological_node_container<'a>(store: &'a Store, node: &str) -> Option<&'a str> {
    store
        .refv(node, "TopologicalNode.ConnectivityNodeContainer")
        .map(|container| resolve_container(store, container))
}

fn equipment_voltage_level<'a>(store: &'a Store, equipment: &str) -> Option<&'a str> {
    store
        .refv(equipment, "Equipment.EquipmentContainer")
        .map(|container| resolve_container(store, container))
        .filter(|container| store.class_of(container) == Some("VoltageLevel"))
}

/// The container a terminal's topology record names: its connectivity node's
/// container, else its topological node's, else the container of its
/// conducting equipment (a junction terminal in a 2.4.15 EQ_BD has no
/// ConnectivityNode and, without TP_BD, no TopologicalNode either).
fn terminal_voltage_level<'a>(store: &'a Store, terminal: &str) -> Option<&'a str> {
    if let Some(node) = store.refv(terminal, "Terminal.ConnectivityNode") {
        return connectivity_node_container(store, node);
    }
    if let Some(node) = store.refv(terminal, "Terminal.TopologicalNode") {
        return topological_node_level(store, node);
    }
    store
        .refv(terminal, "Terminal.ConductingEquipment")
        .and_then(|equipment| store.refv(equipment, "Equipment.EquipmentContainer"))
        .map(|container| resolve_container(store, container))
}

/// The container a TopologicalNode's topology records name. A TopologicalNode
/// whose own container holds none of its ConnectivityNodes (a TP_BD
/// TopologicalNode names one Line while its EQ_BD node names another)
/// follows the nodes' container, because one bus belongs to one container.
fn topological_node_level<'a>(store: &'a Store, topological_node: &str) -> Option<&'a str> {
    let own = topological_node_container(store, topological_node)?;
    let mut members = store.of_class("ConnectivityNode").filter(|node| {
        store.refv(node, "ConnectivityNode.TopologicalNode") == Some(topological_node)
    });
    let Some(first) = members.next() else {
        return Some(own);
    };
    if connectivity_node_container(store, first) == Some(own)
        || members.any(|node| connectivity_node_container(store, node) == Some(own))
    {
        return Some(own);
    }
    connectivity_node_container(store, first).or(Some(own))
}

fn terminal_nominal_kv(mapper: &Mapper<'_>, terminal: &str) -> Option<f64> {
    if let Some(node) = mapper.wiring.node(terminal)
        && let Some(bus) = mapper.bus_of_node.get(node)
    {
        return Some(mapper.kv(*bus));
    }
    let level = terminal_voltage_level(mapper.store, terminal)?;
    let base = mapper.store.refv(level, "VoltageLevel.BaseVoltage")?;
    mapper.store.f(base, "BaseVoltage.nominalVoltage")
}

fn check_equipment_base_voltages(mapper: &mut Mapper<'_>) {
    for object in &mapper.store.objects {
        if !class_is_consumed(&object.class) {
            continue;
        }
        let Some(base) = mapper
            .store
            .refv(&object.id, "ConductingEquipment.BaseVoltage")
        else {
            continue;
        };
        let Some(stated_kv) = mapper.store.f(base, "BaseVoltage.nominalVoltage") else {
            mapper.warnings.push_as(
                &codes::READ_CGMES_FIELD_UNMAPPED,
                format!(
                    "{} `{}` property `ConductingEquipment.BaseVoltage` references BaseVoltage `{base}` without a nominal voltage; fresh CGMES derives the equipment base voltage from its connected voltage level",
                    object.class, object.id
                ),
            );
            continue;
        };
        let mut connected_kv = mapper
            .wiring
            .terminals(&object.id)
            .iter()
            .filter_map(|terminal| terminal_nominal_kv(mapper, terminal))
            .collect::<Vec<_>>();
        connected_kv.sort_by(f64::total_cmp);
        connected_kv.dedup_by(|left, right| {
            (*left - *right).abs() <= 1e-9 * left.abs().max(right.abs()).max(1.0)
        });
        if connected_kv.is_empty() {
            mapper.warnings.push_as(
                &codes::READ_CGMES_FIELD_UNMAPPED,
                format!(
                    "{} `{}` states `ConductingEquipment.BaseVoltage` `{base}` ({stated_kv} kV), but none of its terminals resolve to a voltage level; fresh CGMES cannot reproduce this field",
                    object.class, object.id
                ),
            );
            continue;
        }
        if connected_kv.iter().any(|connected| {
            (*connected - stated_kv).abs() > 1e-9 * connected.abs().max(stated_kv.abs()).max(1.0)
        }) {
            mapper.warnings.push_as(
                &codes::READ_CGMES_VALUE_APPROXIMATED,
                format!(
                    "{} `{}` states `ConductingEquipment.BaseVoltage` {stated_kv} kV, but its connected voltage level value(s) are {connected_kv:?} kV; PowerIO uses the connected voltage levels and fresh CGMES writes their base voltages",
                    object.class, object.id
                ),
            );
        }
    }
}

fn warn_collapsed_base_voltage_identities(store: &Store, warnings: &mut CgmesDiagnostics) {
    let mut by_nominal_voltage: BTreeMap<u64, (f64, Vec<&str>)> = BTreeMap::new();
    for id in store.of_class("BaseVoltage") {
        let Some(nominal_kv) = store
            .f(id, "BaseVoltage.nominalVoltage")
            .filter(|value| value.is_finite() && *value > 0.0)
        else {
            continue;
        };
        by_nominal_voltage
            .entry(nominal_kv.to_bits())
            .or_insert_with(|| (nominal_kv, Vec::new()))
            .1
            .push(id);
    }
    for (_, (nominal_kv, ids)) in by_nominal_voltage {
        if ids.len() < 2 {
            continue;
        }
        let samples = ids.iter().take(5).copied().collect::<Vec<_>>().join("`, `");
        let remainder = if ids.len() > 5 {
            format!(" and {} more", ids.len() - 5)
        } else {
            String::new()
        };
        warnings.push_as(
            &codes::READ_CGMES_VALUE_APPROXIMATED,
            format!(
                "{} distinct BaseVoltage identities [`{samples}`]{remainder} all declare {nominal_kv} kV; PowerIO uses one source neutral voltage value and fresh CGMES emits one deterministic BaseVoltage identity for that voltage",
                ids.len()
            ),
        );
    }
}

fn solved_voltage(store: &Store, topological_node: &str) -> (Option<f64>, Option<f64>) {
    store
        .of_class("SvVoltage")
        .find(|value| store.refv(value, "SvVoltage.TopologicalNode") == Some(topological_node))
        .map_or((None, None), |value| {
            (
                store.f(value, "SvVoltage.v"),
                store.f(value, "SvVoltage.angle"),
            )
        })
}

fn sv_voltage_authority_mismatch<'a>(
    store: &'a Store,
    topological_node: &str,
) -> Option<(&'a str, &'a str, &'a str)> {
    let sv_voltage = store
        .of_class("SvVoltage")
        .find(|value| store.refv(value, "SvVoltage.TopologicalNode") == Some(topological_node))?;
    let sv_authority = store.modeling_authority_set(sv_voltage)?;
    let container = store.refv(
        topological_node,
        "TopologicalNode.ConnectivityNodeContainer",
    )?;
    let container_authority = store.modeling_authority_set(container)?;
    (sv_authority != container_authority).then_some((sv_voltage, sv_authority, container_authority))
}

fn terminal_topological_node<'a>(store: &'a Store, terminal: &str) -> Option<&'a str> {
    store
        .refv(terminal, "Terminal.TopologicalNode")
        .or_else(|| {
            let connectivity_node = store.refv(terminal, "Terminal.ConnectivityNode")?;
            store.refv(connectivity_node, "ConnectivityNode.TopologicalNode")
        })
}

fn boundary_equipment_authorities<'a>(
    store: &'a Store,
    topological_node: &str,
) -> Option<Vec<&'a str>> {
    let container = store.refv(
        topological_node,
        "TopologicalNode.ConnectivityNodeContainer",
    )?;
    if store.class_of(container) != Some("Line") {
        return None;
    }
    let mut authorities = store
        .of_class("Terminal")
        .filter(|terminal| terminal_topological_node(store, terminal) == Some(topological_node))
        .filter_map(|terminal| store.refv(terminal, "Terminal.ConductingEquipment"))
        .filter_map(|equipment| store.modeling_authority_set(equipment))
        .collect::<Vec<_>>();
    authorities.sort_unstable();
    authorities.dedup();
    (authorities.len() > 1).then_some(authorities)
}

fn sv_power_flow_authority_mismatch<'a>(
    store: &'a Store,
    sv_power_flow: &str,
) -> Option<(&'a str, &'a str, &'a str)> {
    let sv_authority = store.modeling_authority_set(sv_power_flow)?;
    let terminal = store.refv(sv_power_flow, "SvPowerFlow.Terminal")?;
    let equipment = store.refv(terminal, "Terminal.ConductingEquipment")?;
    let equipment_authority = store.modeling_authority_set(equipment)?;
    (sv_authority != equipment_authority).then_some((equipment, sv_authority, equipment_authority))
}

fn retain_regulating_control_properties(
    store: &Store,
    equipment: &str,
    properties: &mut BTreeMap<String, String>,
) {
    if let Some(value) = store.text(equipment, "RegulatingCondEq.controlEnabled") {
        properties.insert("RegulatingCondEq.controlEnabled".into(), value.into());
    }
    let Some(control) = store.refv(equipment, super::CGMES_REGULATING_CONTROL_PROPERTY) else {
        return;
    };
    properties.insert(
        super::CGMES_REGULATING_CONTROL_PROPERTY.into(),
        control.into(),
    );
    for property in [
        "RegulatingControl.discrete",
        "RegulatingControl.enabled",
        "RegulatingControl.targetDeadband",
        "RegulatingControl.targetValue",
    ] {
        if let Some(value) = store.text(control, property) {
            properties.insert(property.into(), value.into());
        }
    }
    if let Some(value) = store
        .enum_value(control, "RegulatingControl.mode")
        .map(|value| format!("RegulatingControlModeKind.{value}"))
    {
        properties.insert("RegulatingControl.mode".into(), value);
    }
    if let Some(value) = store
        .enum_value(control, "RegulatingControl.targetValueUnitMultiplier")
        .map(|value| format!("UnitMultiplier.{value}"))
    {
        properties.insert("RegulatingControl.targetValueUnitMultiplier".into(), value);
    }
}

#[allow(clippy::too_many_lines)] // one ordered pass builds every linked topology table
fn build_detailed_connectivity(
    mapper: &mut Mapper<'_>,
    version: CgmesVersion,
    equipment_reactive_limits: Vec<EquipmentReactiveLimits>,
) -> Result<DetailedConnectivity> {
    let store = mapper.store;
    let substations = store
        .of_class("Substation")
        .map(|id| {
            Ok(Substation {
                component: component_id("substation", id)?,
                country: store
                    .enum_value(id, "entsoe:Substation.Country")
                    .map(str::to_string),
                operator: None,
                geographical_tags: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut voltage_levels = store
        .of_class("VoltageLevel")
        .map(|id| {
            let base = store
                .refv(id, "VoltageLevel.BaseVoltage")
                .and_then(|base| store.f(base, "BaseVoltage.nominalVoltage"))
                .unwrap_or(0.0);
            let has_nodes = store
                .of_class("ConnectivityNode")
                .any(|node| connectivity_node_container(store, node) == Some(id));
            let mut buses = match mapper.topology {
                BusSource::TopologicalNodes => store
                    .of_class("TopologicalNode")
                    .filter(|node| topological_node_container(store, node) == Some(id))
                    .filter_map(|node| mapper.bus_of_node.get(node).copied())
                    .collect::<Vec<_>>(),
                BusSource::Calculated => mapper
                    .calculated
                    .iter()
                    .filter(|group| group.container == id)
                    .map(|group| group.bus)
                    .collect::<Vec<_>>(),
            };
            buses.sort_unstable();
            buses.dedup();
            Ok(VoltageLevel {
                component: component_id("voltage_level", id)?,
                substation: store
                    .refv(id, "VoltageLevel.Substation")
                    .map(|substation| component_id("substation", substation))
                    .transpose()?,
                nominal_kv: base,
                low_voltage_limit_kv: store.f(id, "VoltageLevel.lowVoltageLimit"),
                high_voltage_limit_kv: store.f(id, "VoltageLevel.highVoltageLimit"),
                topology_kind: if has_nodes {
                    TopologyKind::NodeBreaker
                } else {
                    TopologyKind::BusBreaker
                },
                buses,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    apply_voltage_limits(store, version, &mut voltage_levels, mapper.warnings);

    let mut connectivity_nodes = Vec::new();
    for id in store.of_class("ConnectivityNode") {
        if let Some(level) = connectivity_node_container(store, id) {
            let calculated_bus = match mapper.topology {
                BusSource::TopologicalNodes => store
                    .refv(id, "ConnectivityNode.TopologicalNode")
                    .and_then(|node| mapper.bus_of_node.get(node).copied()),
                BusSource::Calculated => mapper.bus_of_node.get(id).copied(),
            };
            connectivity_nodes.push(ConnectivityNode {
                component: component_id("connectivity_node", id)?,
                voltage_level: component_id(
                    component_type(store.class_of(level).unwrap_or("")),
                    level,
                )?,
                node_number: None,
                calculated_bus,
            });
        }
    }

    let mut bus_breaker_buses = Vec::new();
    for id in store.of_class("TopologicalNode") {
        if let Some(level) = topological_node_level(store, id) {
            let (voltage_kv, angle_degrees) = solved_voltage(store, id);
            bus_breaker_buses.push(BusBreakerBus {
                component: component_id("bus", id)?,
                voltage_level: component_id(
                    component_type(store.class_of(level).unwrap_or("")),
                    level,
                )?,
                calculated_bus: mapper.bus_of_topological_node(id),
                voltage_kv,
                angle_degrees,
            });
        }
    }

    let mut calculated_buses = Vec::new();
    match mapper.topology {
        BusSource::TopologicalNodes => {
            for id in store.of_class("TopologicalNode") {
                let (Some(own_level), Some(level), Some(calculated_bus)) = (
                    topological_node_container(store, id),
                    topological_node_level(store, id),
                    mapper.bus_of_topological_node(id),
                ) else {
                    continue;
                };
                let members = store
                    .of_class("ConnectivityNode")
                    .filter(|node| store.refv(node, "ConnectivityNode.TopologicalNode") == Some(id))
                    .collect::<Vec<_>>();
                if members.is_empty() {
                    continue;
                }
                if level != own_level {
                    mapper.warnings.push_as(
                        &codes::READ_CGMES_VALUE_APPROXIMATED,
                        format!(
                            "TopologicalNode `{id}` is contained by `{own_level}` while its ConnectivityNodes are contained by `{level}`; the topology records follow the ConnectivityNodes"
                        ),
                    );
                }
                // One record names one container, so nodes joined from another
                // container map to the bus without being listed.
                let nodes = members
                    .iter()
                    .filter(|node| connectivity_node_container(store, node) == Some(level))
                    .map(|node| component_id("connectivity_node", node))
                    .collect::<Result<Vec<_>>>()?;
                let (voltage_kv, angle_degrees) = solved_voltage(store, id);
                calculated_buses.push(CalculatedBus {
                    voltage_level: component_id(
                        component_type(store.class_of(level).unwrap_or("")),
                        level,
                    )?,
                    calculated_bus,
                    nodes,
                    voltage_kv,
                    angle_degrees,
                });
            }
        }
        BusSource::Calculated => {
            for group in &mapper.calculated {
                let nodes = group
                    .nodes
                    .iter()
                    .map(|node| component_id("connectivity_node", node))
                    .collect::<Result<Vec<_>>>()?;
                calculated_buses.push(CalculatedBus {
                    voltage_level: component_id(
                        component_type(store.class_of(&group.container).unwrap_or("")),
                        &group.container,
                    )?,
                    calculated_bus: group.bus,
                    nodes,
                    voltage_kv: None,
                    angle_degrees: None,
                });
            }
        }
    }

    let mut synthesized_busbar_nodes = HashMap::new();
    let mut busbar_sections = Vec::new();
    for id in store.of_class("BusbarSection") {
        let Some(terminal) = mapper.wiring.terminals(id).first() else {
            continue;
        };
        let Some(level) = terminal_voltage_level(store, terminal) else {
            continue;
        };
        let node = if let Some(node) = store.refv(terminal, "Terminal.ConnectivityNode") {
            component_id("connectivity_node", node)?
        } else {
            let Some(topological_node) = store.refv(terminal, "Terminal.TopologicalNode") else {
                mapper.warnings.push(format!(
                    "BusbarSection {} has neither a ConnectivityNode nor a TopologicalNode and was skipped",
                    store.name(id)
                ));
                continue;
            };
            let Some(topological_level) = topological_node_container(store, topological_node)
            else {
                mapper.warnings.push(format!(
                    "BusbarSection {} has no unambiguous VoltageLevel through TopologicalNode {} and was skipped",
                    store.name(id),
                    store.name(topological_node)
                ));
                continue;
            };
            let Some(calculated_bus) = mapper.bus_of_topological_node(topological_node) else {
                mapper.warnings.push(format!(
                    "BusbarSection {} TopologicalNode {} has no calculated bus and was skipped",
                    store.name(id),
                    store.name(topological_node)
                ));
                continue;
            };
            if topological_level != level
                || store.class_of(topological_level) != Some("VoltageLevel")
            {
                mapper.warnings.push(format!(
                    "BusbarSection {} has inconsistent topology and equipment containers and was skipped",
                    store.name(id)
                ));
                continue;
            }
            let node_id = format!("terminal-{terminal}");
            let node = component_id("connectivity_node", &node_id)?;
            connectivity_nodes.push(ConnectivityNode {
                component: node.clone(),
                voltage_level: component_id("voltage_level", topological_level)?,
                node_number: None,
                calculated_bus: Some(calculated_bus),
            });
            synthesized_busbar_nodes.insert((*terminal).clone(), node.clone());
            node
        };
        busbar_sections.push(BusbarSection {
            component: component_id("busbar_section", id)?,
            voltage_level: component_id(
                component_type(store.class_of(level).unwrap_or("")),
                level,
            )?,
            node,
        });
    }

    let junctions = store
        .of_class("Junction")
        .map(|id| {
            Ok(Junction {
                component: component_id("junction", id)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut terminals = Vec::new();
    let mut unplaced_terminals: Vec<String> = Vec::new();
    for id in store.of_class("Terminal") {
        let Some(equipment) = store.refv(id, "Terminal.ConductingEquipment") else {
            continue;
        };
        let Some(level) = terminal_voltage_level(store, id) else {
            continue;
        };
        let sequence = store
            .f(id, "ACDCTerminal.sequenceNumber")
            .or_else(|| store.f(id, "Terminal.sequenceNumber"))
            .unwrap_or(1.0);
        let terminal = u8::try_from(sequence as u64).unwrap_or(u8::MAX);
        let bus = store.refv(id, "Terminal.TopologicalNode");
        let node = store
            .refv(id, "Terminal.ConnectivityNode")
            .map(|value| component_id("connectivity_node", value))
            .transpose()?
            .or_else(|| synthesized_busbar_nodes.get(id).cloned());
        let stated_connected = store.boolean(id, "ACDCTerminal.connected").unwrap_or(true);
        // A terminal that names neither a ConnectivityNode nor a
        // TopologicalNode (a 2.4.15 EQ_BD junction terminal read without
        // TP_BD) sits on no bus, so its record states no connection.
        let unplaced = node.is_none() && bus.is_none();
        if unplaced && stated_connected {
            unplaced_terminals.push(id.to_string());
        }
        let solved_power = mapper.sv_flow.get(id).copied();
        terminals.push(Terminal {
            component: Some(component_id("terminal", id)?),
            equipment: component_id(
                component_type(store.class_of(equipment).unwrap_or("")),
                equipment,
            )?,
            terminal,
            voltage_level: component_id(
                component_type(store.class_of(level).unwrap_or("")),
                level,
            )?,
            bus: bus.map(|value| component_id("bus", value)).transpose()?,
            connectable_bus: bus.map(|value| component_id("bus", value)).transpose()?,
            node,
            connected: stated_connected && !unplaced,
            active_power_mw: solved_power.map(|value| value.0),
            reactive_power_mvar: solved_power.map(|value| value.1),
        });
    }
    if !unplaced_terminals.is_empty() {
        mapper.warnings.push_as(
            &codes::READ_CGMES_RECORD_UNMAPPED,
            format!(
                "{} connected terminal(s) (sample `{}`) reference neither a ConnectivityNode nor a TopologicalNode, so nothing places them on a bus; their records state no connection",
                unplaced_terminals.len(),
                unplaced_terminals[0]
            ),
        );
    }

    let mut switches = Vec::new();
    for class in SWITCH_CLASSES {
        for id in store.of_class(class) {
            let terms = mapper.wiring.terminals(id);
            let (Some(first), Some(second)) = (terms.first(), terms.get(1)) else {
                continue;
            };
            let Some(level) = terminal_voltage_level(store, first) else {
                continue;
            };
            let endpoint = |terminal: &str| -> Result<TopologyEndpoint> {
                if let Some(node) = store.refv(terminal, "Terminal.ConnectivityNode") {
                    Ok(TopologyEndpoint::Node(component_id(
                        "connectivity_node",
                        node,
                    )?))
                } else if let Some(bus) = store.refv(terminal, "Terminal.TopologicalNode") {
                    Ok(TopologyEndpoint::Bus(component_id("bus", bus)?))
                } else {
                    Err(Error::FormatRead {
                        format: FMT,
                        message: format!("switch `{id}` terminal has no topology connection"),
                    })
                }
            };
            switches.push(TopologySwitch {
                component: component_id("switch", id)?,
                voltage_level: component_id(
                    component_type(store.class_of(level).unwrap_or("")),
                    level,
                )?,
                kind: match class {
                    "Disconnector" | "GroundDisconnector" => SwitchKind::Disconnector,
                    "LoadBreakSwitch" => SwitchKind::LoadBreakSwitch,
                    _ => SwitchKind::Breaker,
                },
                endpoint1: endpoint(first)?,
                endpoint2: endpoint(second)?,
                open: store
                    .boolean(id, "Switch.open")
                    .or_else(|| store.boolean(id, "Switch.normalOpen"))
                    .unwrap_or(false),
                retained: store.boolean(id, "Switch.retained").unwrap_or(false),
            });
        }
    }

    terminals.sort_by(|left, right| {
        left.equipment
            .cmp(&right.equipment)
            .then(left.terminal.cmp(&right.terminal))
            .then(left.component.cmp(&right.component))
    });

    let own_output = is_own_output(store);
    let mut component_metadata = store
        .objects
        .iter()
        .filter(|object| !(own_output && writer_derived_component_metadata(&object.class)))
        .map(|object| {
            let mut properties = BTreeMap::new();
            if LOAD_CLASSES.contains(&object.class.as_str())
                || SWITCH_CLASSES.contains(&object.class.as_str())
                || object.class == "ExternalNetworkInjection"
            {
                properties.insert(CGMES_CLASS_PROPERTY.into(), object.class.clone());
            }
            if object.class == "PowerTransformer" {
                properties.insert(CGMES_CLASS_PROPERTY.into(), object.class.clone());
            }
            if object.class == "SeriesCompensator" {
                properties.insert(CGMES_CLASS_PROPERTY.into(), object.class.clone());
                for property in [
                    "SeriesCompensator.r0",
                    "SeriesCompensator.x0",
                    "SeriesCompensator.varistorRatedCurrent",
                    "SeriesCompensator.varistorVoltageThreshold",
                ] {
                    if let Some(value) = store.f(&object.id, property) {
                        properties.insert(property.into(), value.to_string());
                    }
                }
                if let Some(value) = store.boolean(&object.id, "SeriesCompensator.varistorPresent")
                {
                    properties.insert(
                        "SeriesCompensator.varistorPresent".into(),
                        value.to_string(),
                    );
                }
            }
            if object.class == "PowerTransformerEnd" {
                properties.insert(CGMES_CLASS_PROPERTY.into(), object.class.clone());
                if let Some(transformer) =
                    store.refv(&object.id, "PowerTransformerEnd.PowerTransformer")
                {
                    properties.insert(
                        "PowerTransformerEnd.PowerTransformer".into(),
                        transformer.into(),
                    );
                }
                if let Some(end_number) = store.text(&object.id, "TransformerEnd.endNumber") {
                    properties.insert("TransformerEnd.endNumber".into(), end_number.into());
                }
            }
            if object.class == "SynchronousMachine"
                && let Some(unit) = store.refv(&object.id, "RotatingMachine.GeneratingUnit")
            {
                properties.insert(super::CGMES_GENERATING_UNIT_PROPERTY.into(), unit.into());
            }
            if object.class == "SynchronousMachine" {
                for (property, enumeration) in [
                    ("SynchronousMachine.type", "SynchronousMachineKind"),
                    (
                        "SynchronousMachine.operatingMode",
                        "SynchronousMachineOperatingMode",
                    ),
                ] {
                    if let Some(value) = store
                        .enum_value(&object.id, property)
                        .map(|value| format!("{enumeration}.{value}"))
                    {
                        properties.insert(property.into(), value);
                    }
                }
                if let Some(value) = store.text(&object.id, "SynchronousMachine.referencePriority")
                {
                    properties.insert("SynchronousMachine.referencePriority".into(), value.into());
                }
            }
            retain_regulating_control_properties(store, &object.id, &mut properties);
            if object.class.ends_with("GeneratingUnit") {
                properties.insert(CGMES_CLASS_PROPERTY.into(), object.class.clone());
                for property in [
                    "IdentifiedObject.description",
                    "GeneratingUnit.initialP",
                    "GeneratingUnit.nominalP",
                ] {
                    if let Some(value) = store.text(&object.id, property) {
                        properties.insert(property.into(), value.into());
                    }
                }
                if let Some(value) = store.boolean(&object.id, "Equipment.aggregate") {
                    properties.insert("Equipment.aggregate".into(), value.to_string());
                }
                if let Some(value) = store
                    .enum_value(&object.id, "GeneratingUnit.genControlSource")
                    .map(|value| format!("GeneratorControlSource.{value}"))
                {
                    properties.insert("GeneratingUnit.genControlSource".into(), value);
                }
            }
            if let Some(in_service) = mapper.sv_status.get(&object.id) {
                properties.insert(
                    super::CGMES_SV_STATUS_PROPERTY.into(),
                    in_service.to_string(),
                );
            }
            if object.class == "TopologicalNode"
                && sv_voltage_authority_mismatch(store, &object.id).is_some()
            {
                properties.insert(
                    super::CGMES_SV_VOLTAGE_AUTHORITY_MISMATCH_PROPERTY.into(),
                    "true".into(),
                );
            }
            Ok(ComponentMetadata {
                component: component_id(component_type(&object.class), &object.id)?,
                name: store
                    .text(&object.id, "IdentifiedObject.name")
                    .map(str::to_string),
                equipment_container: store
                    .refv(&object.id, "Equipment.EquipmentContainer")
                    .map(|container| {
                        component_id(
                            component_type(store.class_of(container).unwrap_or_default()),
                            container,
                        )
                    })
                    .transpose()?,
                aliases: store
                    .text(&object.id, "IdentifiedObject.shortName")
                    .map(|value| {
                        vec![ComponentAlias {
                            value: value.to_string(),
                            alias_type: Some("short_name".into()),
                        }]
                    })
                    .unwrap_or_default(),
                external_identifiers: vec![ExternalIdentifier {
                    value: object.id.clone(),
                    authority: Some("CGMES".into()),
                }],
                properties,
                fictitious: store
                    .boolean(&object.id, "IdentifiedObject.isFictitious")
                    .unwrap_or(false),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    component_metadata.sort_by(|left, right| left.component.cmp(&right.component));

    let mut operational_limit_groups = read_operational_limit_groups(mapper, version)?;
    operational_limit_groups.sort_by(|left, right| {
        left.equipment
            .cmp(&right.equipment)
            .then(left.terminal.cmp(&right.terminal))
            .then(left.id.cmp(&right.id))
    });
    let tap_changers = read_tap_changers(mapper)?;
    let dc = read_dc_equipment(mapper, version)?;

    Ok(DetailedConnectivity {
        omitted_fields: Vec::new(),
        component_metadata,
        subnetworks: Vec::new(),
        substations,
        voltage_levels,
        bus_breaker_buses,
        calculated_buses,
        connectivity_nodes,
        busbar_sections,
        junctions,
        terminals,
        switches,
        internal_connections: Vec::new(),
        operational_limit_groups,
        tap_changers,
        equipment_reactive_limits,
        boundary_lines: Vec::new(),
        tie_lines: Vec::new(),
        dc_converter_units: dc.converter_units,
        dc_topological_nodes: dc.topological_nodes,
        dc_nodes: dc.nodes,
        dc_grounds: dc.grounds,
        dc_busbars: dc.busbars,
        dc_lines: dc.lines,
        dc_series_devices: dc.series_devices,
        dc_switches: dc.switches,
        voltage_source_converters: dc.voltage_source_converters,
        line_commutated_converters: dc.line_commutated_converters,
    })
}

#[derive(Default)]
struct ReadDcEquipment {
    converter_units: Vec<DcConverterUnit>,
    topological_nodes: Vec<DcTopologicalNode>,
    nodes: Vec<DcNode>,
    grounds: Vec<DcGround>,
    busbars: Vec<DcBusbar>,
    lines: Vec<DcLine>,
    series_devices: Vec<DcSeriesDevice>,
    switches: Vec<DcSwitch>,
    voltage_source_converters: Vec<VoltageSourceConverter>,
    line_commutated_converters: Vec<LineCommutatedConverter>,
}

struct DcTerminalWiring {
    by_equipment: HashMap<String, Vec<String>>,
}

impl DcTerminalWiring {
    fn build(store: &Store) -> Result<Self> {
        let mut by_equipment = HashMap::<String, Vec<(Option<u32>, String)>>::new();
        for class in ["DCTerminal", "ACDCConverterDCTerminal"] {
            for terminal in store.of_class(class) {
                let equipment_property = if class == "DCTerminal" {
                    "DCTerminal.DCConductingEquipment"
                } else {
                    "ACDCConverterDCTerminal.DCConductingEquipment"
                };
                if let Some(equipment) = store.refv(terminal, equipment_property) {
                    let sequence = optional_u32(store, terminal, "ACDCTerminal.sequenceNumber")?;
                    by_equipment
                        .entry(equipment.to_string())
                        .or_default()
                        .push((sequence, terminal.to_string()));
                }
            }
        }
        Ok(Self {
            by_equipment: by_equipment
                .into_iter()
                .map(|(equipment, mut terminals)| {
                    terminals.sort_by(|left, right| match (left.0, right.0) {
                        (Some(left), Some(right)) => left.cmp(&right),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => left.1.cmp(&right.1),
                    });
                    (
                        equipment,
                        terminals
                            .into_iter()
                            .map(|(_, terminal)| terminal)
                            .collect(),
                    )
                })
                .collect(),
        })
    }

    fn terminals(&self, equipment: &str) -> &[String] {
        self.by_equipment.get(equipment).map_or(&[], Vec::as_slice)
    }
}

fn dc_terminal(
    store: &Store,
    wiring: &DcTerminalWiring,
    equipment: &str,
    index: usize,
) -> Result<Option<DcTerminal>> {
    let Some(terminal) = wiring.terminals(equipment).get(index) else {
        return Ok(None);
    };
    Ok(Some(DcTerminal {
        component: Some(component_id("dc_terminal", terminal)?),
        sequence_number: optional_u32(store, terminal, "ACDCTerminal.sequenceNumber")?,
        dc_node: store
            .refv(terminal, "DCBaseTerminal.DCNode")
            .map(|node| component_id("dc_node", node))
            .transpose()?,
        dc_topological_node: store
            .refv(terminal, "DCBaseTerminal.DCTopologicalNode")
            .map(|node| component_id("dc_topological_node", node))
            .transpose()?,
        polarity: match store.enum_value(terminal, "ACDCConverterDCTerminal.polarity") {
            Some("positive") => Some(DcPolarity::Positive),
            Some("middle") => Some(DcPolarity::Middle),
            Some("negative") => Some(DcPolarity::Negative),
            Some(value) => {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "ACDCConverterDCTerminal `{terminal}` has unknown polarity `{value}`"
                    ),
                });
            }
            None => None,
        },
        connected: store.boolean(terminal, "ACDCTerminal.connected"),
        active_power_mw: None,
        current_a: None,
    }))
}

fn converter_pcc_terminal(store: &Store, converter: &str) -> Result<Option<TerminalReference>> {
    let Some(terminal) = store.refv(converter, "ACDCConverter.PccTerminal") else {
        return Ok(None);
    };
    let Some(equipment) = store.refv(terminal, "Terminal.ConductingEquipment") else {
        return Ok(None);
    };
    let Some(sequence) = optional_u32(store, terminal, "ACDCTerminal.sequenceNumber")? else {
        return Ok(None);
    };
    let terminal_number = u8::try_from(sequence).map_err(|_| Error::FormatRead {
        format: FMT,
        message: format!("Terminal `{terminal}` sequence number {sequence} exceeds 255"),
    })?;
    Ok(Some(TerminalReference {
        equipment: component_id(
            component_type(store.class_of(equipment).unwrap_or_default()),
            equipment,
        )?,
        terminal: terminal_number,
    }))
}

fn converter_control_mode(
    store: &Store,
    converter: &str,
    class: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<AcDcConverterControlMode> {
    let value = store.enum_value(converter, &format!("{class}.pPccControl"));
    match (class, value) {
        ("VsConverter", Some("udc")) | ("CsConverter", Some("dcVoltage")) => {
            Some(AcDcConverterControlMode::DcVoltage)
        }
        ("VsConverter", Some("pPcc")) | ("CsConverter", Some("activePower")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPcc)
        }
        ("VsConverter", Some("pPccAndUdcDroop")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop)
        }
        ("VsConverter", Some("pPccAndUdcDroopWithCompensation")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation)
        }
        ("VsConverter", Some("pPccAndUdcDroopPilot")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot)
        }
        ("CsConverter", Some("dcCurrent")) => Some(AcDcConverterControlMode::DcCurrent),
        (_, Some(other)) => {
            warnings.push(format!(
                "{class} {}: {class}.pPccControl `{other}` is unknown and was not assigned",
                store.name(converter)
            ));
            None
        }
        (_, None) => None,
    }
}

fn vsc_reactive_limits(
    store: &Store,
    converter: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<ReactiveLimits> {
    let curve = store.refv(converter, "VsConverter.CapabilityCurve")?;
    read_reactive_capability_curve(store, curve, "VsCapabilityCurve", warnings)
        .map(ReactiveLimits::CapabilityCurve)
}

fn synchronous_machine_reactive_limits(
    store: &Store,
    machine: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<ReactiveLimits> {
    let curve = store.refv(machine, "SynchronousMachine.InitialReactiveCapabilityCurve")?;
    read_reactive_capability_curve(store, curve, "ReactiveCapabilityCurve", warnings)
        .map(ReactiveLimits::CapabilityCurve)
}

fn read_reactive_capability_curve(
    store: &Store,
    curve: &str,
    class: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<ReactiveCapabilityCurve> {
    let curve_style = match store.enum_value(curve, "Curve.curveStyle") {
        Some("constantYValue") => CurveStyle::ConstantYValue,
        Some("straightLineYValues") => CurveStyle::StraightLineYValues,
        Some(value) => {
            warnings.push(format!(
                "{class} {}: Curve.curveStyle `{value}` is unknown and reactive limits were not assigned",
                store.name(curve)
            ));
            return None;
        }
        None => {
            warnings.push(format!(
                "{class} {}: required Curve.curveStyle is absent and reactive limits were not assigned",
                store.name(curve)
            ));
            return None;
        }
    };
    for (property, expected) in [
        ("Curve.xUnit", "W"),
        ("Curve.y1Unit", "VAr"),
        ("Curve.y2Unit", "VAr"),
    ] {
        match store.enum_value(curve, property) {
            Some(value) if value == expected => {}
            Some(value) => {
                warnings.push(format!(
                    "{class} {}: {property} `UnitSymbol.{value}` is unsupported; expected `UnitSymbol.{expected}`, so reactive limits were not assigned",
                    store.name(curve)
                ));
                return None;
            }
            None => {
                warnings.push(format!(
                    "{class} {}: {property} is absent; expected `UnitSymbol.{expected}`, so reactive limits were not assigned",
                    store.name(curve)
                ));
                return None;
            }
        }
    }
    let mut points = {
        store
            .of_class("CurveData")
            .filter(|point| store.refv(point, "CurveData.Curve") == Some(curve))
            .filter_map(|point| {
                Some(ReactiveCapabilityCurvePoint {
                    active_power_mw: store.f(point, "CurveData.xvalue")?,
                    minimum_reactive_power_mvar: store.f(point, "CurveData.y1value")?,
                    maximum_reactive_power_mvar: store.f(point, "CurveData.y2value")?,
                    properties: BTreeMap::new(),
                })
            })
            .collect::<Vec<_>>()
    };
    points.sort_by(|left, right| left.active_power_mw.total_cmp(&right.active_power_mw));
    Some(ReactiveCapabilityCurve {
        curve_style,
        properties: BTreeMap::new(),
        points,
    })
}

fn optional_u32(store: &Store, id: &str, property: &str) -> Result<Option<u32>> {
    let Some(value) = store.f(id, property) else {
        return Ok(None);
    };
    if value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!("{id} property {property} is not a valid unsigned integer: {value}"),
        });
    }
    Ok(Some(value as u32))
}

fn optional_component_reference(
    store: &Store,
    id: &str,
    property: &str,
) -> Result<Option<ComponentId>> {
    store
        .refv(id, property)
        .map(|target| {
            component_id(
                component_type(store.class_of(target).unwrap_or_default()),
                target,
            )
        })
        .transpose()
}

fn dc_converter_operating_mode(store: &Store, id: &str) -> Result<DcConverterOperatingMode> {
    match store.enum_value(id, "DCConverterUnit.operationMode") {
        Some("bipolar") => Ok(DcConverterOperatingMode::Bipolar),
        Some("monopolarGroundReturn") => Ok(DcConverterOperatingMode::MonopolarGroundReturn),
        Some("monopolarMetallicReturn") => Ok(DcConverterOperatingMode::MonopolarMetallicReturn),
        Some(value) => Err(Error::FormatRead {
            format: FMT,
            message: format!("DCConverterUnit `{id}` has unknown operation mode `{value}`"),
        }),
        None => Err(Error::FormatRead {
            format: FMT,
            message: format!("DCConverterUnit `{id}` has no DCConverterUnit.operationMode"),
        }),
    }
}

fn lcc_operating_mode(
    store: &Store,
    id: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<LineCommutatedConverterOperatingMode> {
    match store.enum_value(id, "CsConverter.operatingMode") {
        Some("rectifier") => Some(LineCommutatedConverterOperatingMode::Rectifier),
        Some("inverter") => Some(LineCommutatedConverterOperatingMode::Inverter),
        Some(value) => {
            warnings.push(format!(
                "CsConverter {}: CsConverter.operatingMode `{value}` is unknown and was not assigned",
                store.name(id)
            ));
            None
        }
        None => None,
    }
}

fn vsc_voltage_regulator_on(
    store: &Store,
    id: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<bool> {
    match store.enum_value(id, "VsConverter.qPccControl") {
        Some("voltagePcc") => Some(true),
        Some("reactivePcc") => Some(false),
        Some(value) => {
            warnings.push(format!(
                "VsConverter {}: VsConverter.qPccControl `{value}` is unknown and was not assigned",
                store.name(id)
            ));
            None
        }
        None => None,
    }
}

fn cgmes_2_unit_converter_rated_dc_voltage(
    store: &Store,
    equipment_class: &str,
    equipment: &str,
    unit: &str,
) -> Result<f64> {
    let mut values = Vec::new();
    for class in ["VsConverter", "CsConverter"] {
        for converter in store
            .of_class(class)
            .filter(|converter| store.refv(converter, "Equipment.EquipmentContainer") == Some(unit))
        {
            let Some(value) = store.f(converter, "ACDCConverter.ratedUdc") else {
                continue;
            };
            if !value.is_finite() || value <= 0.0 {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "CGMES 2.4.15 {equipment_class} `{equipment}` has no DCConductingEquipment.ratedUdc; converter `{converter}` in DCConverterUnit `{unit}` has nonpositive or nonfinite ACDCConverter.ratedUdc `{value}`"
                    ),
                });
            }
            if !values
                .iter()
                .any(|existing: &f64| existing.to_bits() == value.to_bits())
            {
                values.push(value);
            }
        }
    }

    let value = match values.as_slice() {
        [] => {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 {equipment_class} `{equipment}` has no DCConductingEquipment.ratedUdc and DCConverterUnit `{unit}` has no explicit positive ACDCConverter.ratedUdc"
                ),
            });
        }
        [value] => *value,
        _ => {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 {equipment_class} `{equipment}` has no DCConductingEquipment.ratedUdc and DCConverterUnit `{unit}` has conflicting ACDCConverter.ratedUdc values: {}",
                    values
                        .iter()
                        .map(f64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    };
    Ok(value)
}

fn cgmes_2_ground_rated_dc_voltage(
    store: &Store,
    ground: &str,
    warnings: &mut CgmesDiagnostics,
) -> Result<f64> {
    let unit = store
        .refv(ground, "Equipment.EquipmentContainer")
        .filter(|container| store.class_of(container) == Some("DCConverterUnit"))
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: format!(
                "CGMES 2.4.15 DCGround `{ground}` has no DCConductingEquipment.ratedUdc and is not contained by a DCConverterUnit"
            ),
        })?;
    let value = cgmes_2_unit_converter_rated_dc_voltage(store, "DCGround", ground, unit)?;
    warnings.push_as(&codes::READ_CGMES_VALUE_APPROXIMATED, format!(
        "CGMES 2.4.15 DCGround `{ground}` has no DCConductingEquipment.ratedUdc; derived {value} kV from the unique positive ACDCConverter.ratedUdc in DCConverterUnit `{unit}`"
    ));
    Ok(value)
}

fn cgmes_2_line_rated_dc_voltage(
    store: &Store,
    wiring: &DcTerminalWiring,
    line: &str,
    warnings: &mut CgmesDiagnostics,
) -> Result<f64> {
    let terminals = wiring.terminals(line);
    if terminals.len() != 2 {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!(
                "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc and does not have exactly two DC terminals"
            ),
        });
    }

    let mut endpoints = Vec::with_capacity(2);
    for terminal in terminals {
        let (node, container_property) = if let Some(node) =
            store.refv(terminal, "DCBaseTerminal.DCNode")
        {
            (node, "DCNode.DCEquipmentContainer")
        } else if let Some(node) = store.refv(terminal, "DCBaseTerminal.DCTopologicalNode") {
            (node, "DCTopologicalNode.DCEquipmentContainer")
        } else {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; terminal `{terminal}` has neither DCBaseTerminal.DCNode nor DCBaseTerminal.DCTopologicalNode"
                ),
            });
        };
        let unit = store
            .refv(node, container_property)
            .filter(|container| store.class_of(container) == Some("DCConverterUnit"))
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; endpoint node `{node}` is not contained by a DCConverterUnit"
                ),
            })?;
        let value = cgmes_2_unit_converter_rated_dc_voltage(store, "DCLineSegment", line, unit)?;
        endpoints.push((terminal.as_str(), node, unit, value));
    }

    let first = endpoints[0];
    let second = endpoints[1];
    if first.3.to_bits() != second.3.to_bits() {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!(
                "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; endpoint DCConverterUnit `{}` has ACDCConverter.ratedUdc {} kV but endpoint DCConverterUnit `{}` has {} kV",
                first.2, first.3, second.2, second.3
            ),
        });
    }

    warnings.push_as(&codes::READ_CGMES_VALUE_APPROXIMATED, format!(
        "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; derived {} kV because terminal `{}` node `{}` belongs to DCConverterUnit `{}` and terminal `{}` node `{}` belongs to DCConverterUnit `{}`, and both units have the same unique positive ACDCConverter.ratedUdc",
        first.3, first.0, first.1, first.2, second.0, second.1, second.2
    ));
    Ok(first.3)
}

#[allow(clippy::too_many_lines)]
fn read_dc_equipment(mapper: &mut Mapper<'_>, version: CgmesVersion) -> Result<ReadDcEquipment> {
    let store = mapper.store;
    let wiring = DcTerminalWiring::build(store)?;
    let mut result = ReadDcEquipment::default();

    for id in store.of_class("DCConverterUnit") {
        result.converter_units.push(DcConverterUnit {
            component: component_id("dc_converter_unit", id)?,
            substation: store
                .refv(id, "DCConverterUnit.Substation")
                .map(|substation| component_id("substation", substation))
                .transpose()?,
            operation_mode: dc_converter_operating_mode(store, id)?,
        });
    }

    for id in store.of_class("DCTopologicalNode") {
        result.topological_nodes.push(DcTopologicalNode {
            component: component_id("dc_topological_node", id)?,
            dc_converter_unit: store
                .refv(id, "DCTopologicalNode.DCEquipmentContainer")
                .map(|unit| component_id("dc_converter_unit", unit))
                .transpose()?,
        });
    }

    for id in store.of_class("DCNode") {
        result.nodes.push(DcNode {
            component: component_id("dc_node", id)?,
            nominal_voltage_kv: None,
            dc_converter_unit: store
                .refv(id, "DCNode.DCEquipmentContainer")
                .map(|unit| component_id("dc_converter_unit", unit))
                .transpose()?,
            dc_topological_node: store
                .refv(id, "DCNode.DCTopologicalNode")
                .map(|node| component_id("dc_topological_node", node))
                .transpose()?,
            voltage_kv: None,
        });
    }

    for id in store.of_class("DCGround") {
        let Some(dc_terminal) = dc_terminal(store, &wiring, id, 0)? else {
            mapper.warnings.push(format!(
                "DCGround {}: missing DC terminal; skipped",
                store.name(id)
            ));
            continue;
        };
        let rated_dc_voltage_kv = store.f(id, "DCConductingEquipment.ratedUdc");
        let rated_dc_voltage_kv =
            if rated_dc_voltage_kv.is_none() && version == CgmesVersion::V2_4_15 {
                Some(cgmes_2_ground_rated_dc_voltage(store, id, mapper.warnings)?)
            } else {
                rated_dc_voltage_kv
            };
        result.grounds.push(DcGround {
            component: component_id("dc_ground", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal,
            rated_dc_voltage_kv,
            resistance_ohm: store.f(id, "DCGround.r"),
            inductance_h: store.f(id, "DCGround.inductance"),
        });
    }
    for id in store.of_class("DCBusbar") {
        let Some(dc_terminal) = dc_terminal(store, &wiring, id, 0)? else {
            mapper.warnings.push(format!(
                "DCBusbar {}: missing DC terminal; skipped",
                store.name(id)
            ));
            continue;
        };
        result.busbars.push(DcBusbar {
            component: component_id("dc_busbar", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal,
            rated_dc_voltage_kv: store.f(id, "DCConductingEquipment.ratedUdc"),
        });
    }
    for id in store.of_class("DCLineSegment") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "DCLineSegment {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        let rated_dc_voltage_kv = store.f(id, "DCConductingEquipment.ratedUdc");
        let rated_dc_voltage_kv =
            if rated_dc_voltage_kv.is_none() && version == CgmesVersion::V2_4_15 {
                Some(cgmes_2_line_rated_dc_voltage(
                    store,
                    &wiring,
                    id,
                    mapper.warnings,
                )?)
            } else {
                rated_dc_voltage_kv
            };
        result.lines.push(DcLine {
            component: component_id("dc_line", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal1,
            dc_terminal2,
            rated_dc_voltage_kv,
            resistance_ohm: store.f(id, "DCLineSegment.resistance"),
            inductance_h: store.f(id, "DCLineSegment.inductance"),
            capacitance_f: store.f(id, "DCLineSegment.capacitance"),
            length_km: store.f(id, "DCLineSegment.length"),
        });
    }
    for id in store.of_class("DCSeriesDevice") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "DCSeriesDevice {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        result.series_devices.push(DcSeriesDevice {
            component: component_id("dc_series_device", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal1,
            dc_terminal2,
            rated_dc_voltage_kv: store
                .f(id, "DCConductingEquipment.ratedUdc")
                .or_else(|| store.f(id, "DCSeriesDevice.ratedUdc")),
            resistance_ohm: store.f(id, "DCSeriesDevice.resistance"),
            inductance_h: store.f(id, "DCSeriesDevice.inductance"),
        });
    }
    for class in ["DCSwitch", "DCBreaker", "DCDisconnector"] {
        for id in store.of_class(class) {
            let (Some(first), Some(second)) = (
                dc_terminal(store, &wiring, id, 0)?,
                dc_terminal(store, &wiring, id, 1)?,
            ) else {
                mapper.warnings.push(format!(
                    "{class} {}: fewer than two DC terminals; skipped",
                    store.name(id)
                ));
                continue;
            };
            let open =
                store
                    .boolean(id, "Switch.open")
                    .or(match (first.connected, second.connected) {
                        (Some(first), Some(second)) => Some(!first || !second),
                        (Some(connected), None) | (None, Some(connected)) => Some(!connected),
                        (None, None) => None,
                    });
            result.switches.push(DcSwitch {
                component: component_id("dc_switch", id)?,
                equipment_container: optional_component_reference(
                    store,
                    id,
                    "Equipment.EquipmentContainer",
                )?,
                dc_terminal1: first,
                dc_terminal2: second,
                kind: match class {
                    "DCSwitch" => DcSwitchKind::Switch,
                    "DCBreaker" => DcSwitchKind::Breaker,
                    _ => DcSwitchKind::Disconnector,
                },
                rated_dc_voltage_kv: store.f(id, "DCConductingEquipment.ratedUdc"),
                open,
                resistance_ohm: None,
            });
        }
    }

    for id in store.of_class("VsConverter") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "VsConverter {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        result
            .voltage_source_converters
            .push(VoltageSourceConverter {
                component: component_id("voltage_source_converter", id)?,
                dc_converter_unit: optional_component_reference(
                    store,
                    id,
                    "Equipment.EquipmentContainer",
                )?,
                dc_terminal1,
                dc_terminal2,
                base_apparent_power_mva: store.f(id, "ACDCConverter.baseS"),
                minimum_active_power_mw: store.f(id, "ACDCConverter.minP"),
                maximum_active_power_mw: store.f(id, "ACDCConverter.maxP"),
                minimum_dc_voltage_kv: store.f(id, "ACDCConverter.minUdc"),
                maximum_dc_voltage_kv: store.f(id, "ACDCConverter.maxUdc"),
                rated_dc_voltage_kv: store.f(id, "ACDCConverter.ratedUdc"),
                valve_u0_kv: store.f(id, "ACDCConverter.valveU0"),
                number_of_valves: optional_u32(store, id, "ACDCConverter.numberOfValves")?,
                idle_loss_mw: store.f(id, "ACDCConverter.idleLoss"),
                switching_loss_mw_per_ampere: store.f(id, "ACDCConverter.switchingLoss"),
                resistive_loss_ohm: store.f(id, "ACDCConverter.resistiveLoss"),
                control_mode: converter_control_mode(store, id, "VsConverter", mapper.warnings),
                active_power_at_pcc_mw: store.f(id, "ACDCConverter.p"),
                reactive_power_at_pcc_mvar: store.f(id, "ACDCConverter.q"),
                target_active_power_mw: store.f(id, "ACDCConverter.targetPpcc"),
                target_dc_voltage_kv: store.f(id, "ACDCConverter.targetUdc"),
                pcc_terminal: converter_pcc_terminal(store, id)?,
                droop_curve: None,
                droop: store.f(id, "VsConverter.droop"),
                droop_compensation: store.f(id, "VsConverter.droopCompensation"),
                q_share: store.f(id, "VsConverter.qShare"),
                maximum_modulation_index: store.f(id, "VsConverter.maxModulationIndex"),
                maximum_valve_current_a: store.f(id, "VsConverter.maxValveCurrent"),
                voltage_regulator_on: vsc_voltage_regulator_on(store, id, mapper.warnings),
                voltage_setpoint_kv: store.f(id, "VsConverter.targetUpcc"),
                reactive_power_setpoint_mvar: store.f(id, "VsConverter.targetQpcc"),
                reactive_limits: vsc_reactive_limits(store, id, mapper.warnings),
                pole_loss_active_power_mw: store.f(id, "ACDCConverter.poleLossP"),
                dc_current_a: store.f(id, "ACDCConverter.idc"),
                ac_voltage_kv: store.f(id, "ACDCConverter.uc"),
                dc_voltage_kv: store.f(id, "ACDCConverter.udc"),
                delta_degrees: store.f(id, "VsConverter.delta"),
                uf_kv: store.f(id, "VsConverter.uf"),
                uv_kv: store.f(id, "VsConverter.uv"),
            });
    }
    for id in store.of_class("CsConverter") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "CsConverter {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        result
            .line_commutated_converters
            .push(LineCommutatedConverter {
                component: component_id("line_commutated_converter", id)?,
                dc_converter_unit: optional_component_reference(
                    store,
                    id,
                    "Equipment.EquipmentContainer",
                )?,
                dc_terminal1,
                dc_terminal2,
                base_apparent_power_mva: store.f(id, "ACDCConverter.baseS"),
                minimum_active_power_mw: store.f(id, "ACDCConverter.minP"),
                maximum_active_power_mw: store.f(id, "ACDCConverter.maxP"),
                minimum_dc_voltage_kv: store.f(id, "ACDCConverter.minUdc"),
                maximum_dc_voltage_kv: store.f(id, "ACDCConverter.maxUdc"),
                rated_dc_voltage_kv: store.f(id, "ACDCConverter.ratedUdc"),
                valve_u0_kv: store.f(id, "ACDCConverter.valveU0"),
                number_of_valves: optional_u32(store, id, "ACDCConverter.numberOfValves")?,
                idle_loss_mw: store.f(id, "ACDCConverter.idleLoss"),
                switching_loss_mw_per_ampere: store.f(id, "ACDCConverter.switchingLoss"),
                resistive_loss_ohm: store.f(id, "ACDCConverter.resistiveLoss"),
                control_mode: converter_control_mode(store, id, "CsConverter", mapper.warnings),
                active_power_at_pcc_mw: store.f(id, "ACDCConverter.p"),
                reactive_power_at_pcc_mvar: store.f(id, "ACDCConverter.q"),
                target_active_power_mw: store.f(id, "ACDCConverter.targetPpcc"),
                target_dc_voltage_kv: store.f(id, "ACDCConverter.targetUdc"),
                pcc_terminal: converter_pcc_terminal(store, id)?,
                droop_curve: None,
                reactive_model: None,
                power_factor: None,
                operating_mode: lcc_operating_mode(store, id, mapper.warnings),
                rated_dc_current_a: store.f(id, "CsConverter.ratedIdc"),
                minimum_alpha_degrees: store.f(id, "CsConverter.minAlpha"),
                maximum_alpha_degrees: store.f(id, "CsConverter.maxAlpha"),
                minimum_gamma_degrees: store.f(id, "CsConverter.minGamma"),
                maximum_gamma_degrees: store.f(id, "CsConverter.maxGamma"),
                target_alpha_degrees: store.f(id, "CsConverter.targetAlpha"),
                target_gamma_degrees: store.f(id, "CsConverter.targetGamma"),
                target_dc_current_a: store.f(id, "CsConverter.targetIdc"),
                pole_loss_active_power_mw: store.f(id, "ACDCConverter.poleLossP"),
                dc_current_a: store.f(id, "ACDCConverter.idc"),
                ac_voltage_kv: store.f(id, "ACDCConverter.uc"),
                dc_voltage_kv: store.f(id, "ACDCConverter.udc"),
                alpha_degrees: store.f(id, "CsConverter.alpha"),
                gamma_degrees: store.f(id, "CsConverter.gamma"),
            });
    }
    Ok(result)
}

fn terminal_reference(store: &Store, terminal: &str) -> Result<Option<TerminalReference>> {
    let Some(equipment) = store.refv(terminal, "Terminal.ConductingEquipment") else {
        return Ok(None);
    };
    let sequence = store
        .f(terminal, "ACDCTerminal.sequenceNumber")
        .or_else(|| store.f(terminal, "Terminal.sequenceNumber"))
        .unwrap_or(1.0);
    let terminal = u8::try_from(sequence.round() as u64).unwrap_or(u8::MAX);
    Ok(Some(TerminalReference {
        equipment: component_id(
            component_type(store.class_of(equipment).unwrap_or_default()),
            equipment,
        )?,
        terminal,
    }))
}

fn read_static_var_compensators(mapper: &mut Mapper<'_>) -> Result<Vec<StaticVarCompensator>> {
    let mut values = Vec::new();
    for id in mapper.store.of_class("StaticVarCompensator") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            mapper.warnings.push(format!(
                "StaticVarCompensator {}: no terminal on a topological node; skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let store = mapper.store;
        let inductive_rating = store
            .f(id, "StaticVarCompensator.inductiveRating")
            .unwrap_or(0.0);
        let capacitive_rating = store
            .f(id, "StaticVarCompensator.capacitiveRating")
            .unwrap_or(0.0);
        let mut svc = StaticVarCompensator::new(
            bus,
            if inductive_rating == 0.0 {
                0.0
            } else {
                1.0 / inductive_rating
            },
            if capacitive_rating == 0.0 {
                0.0
            } else {
                1.0 / capacitive_rating
            },
        );
        if inductive_rating == 0.0 || capacitive_rating == 0.0 {
            mapper.warnings.push(format!(
                "StaticVarCompensator {}: zero or missing inductive/capacitive rating has no finite susceptance; using 0 S for that bound",
                store.name(id)
            ));
        }

        let control = store.refv(id, "RegulatingCondEq.RegulatingControl");
        let mode = control
            .and_then(|value| store.enum_value(value, "RegulatingControl.mode"))
            .or_else(|| store.enum_value(id, "StaticVarCompensator.sVCControlMode"));
        svc.regulation_mode =
            if mode.is_some_and(|value| value.eq_ignore_ascii_case("reactivePower")) {
                StaticVarCompensatorRegulationMode::ReactivePower
            } else {
                StaticVarCompensatorRegulationMode::Voltage
            };
        let target = control.and_then(|value| store.f(value, "RegulatingControl.targetValue"));
        svc.voltage_setpoint_kv =
            if svc.regulation_mode == StaticVarCompensatorRegulationMode::Voltage {
                control
                    .and_then(|value| regulating_control_target_kv(store, value, mapper.warnings))
                    .or_else(|| store.f(id, "StaticVarCompensator.voltageSetPoint"))
                    .unwrap_or(0.0)
            } else {
                store
                    .f(id, "StaticVarCompensator.voltageSetPoint")
                    .unwrap_or(0.0)
            };
        svc.reactive_power_setpoint_mvar =
            if svc.regulation_mode == StaticVarCompensatorRegulationMode::ReactivePower {
                target.unwrap_or_else(|| store.f(id, "StaticVarCompensator.q").unwrap_or(0.0))
            } else {
                0.0
            };
        let equipment_enabled = store
            .boolean(id, "RegulatingCondEq.controlEnabled")
            .unwrap_or(false);
        let control_enabled = control
            .and_then(|value| store.boolean(value, "RegulatingControl.enabled"))
            .unwrap_or(equipment_enabled);
        svc.regulating = equipment_enabled && control_enabled;
        svc.regulating_terminal = control
            .and_then(|value| store.refv(value, "RegulatingControl.Terminal"))
            .map(|terminal| terminal_reference(store, terminal))
            .transpose()?
            .flatten();
        let flow = mapper
            .wiring
            .terminals(id)
            .iter()
            .find_map(|terminal| mapper.sv_flow.get(terminal).copied());
        svc.p = store
            .f(id, "StaticVarCompensator.p")
            .or_else(|| flow.map(|value| value.0))
            .unwrap_or(0.0);
        svc.q = store
            .f(id, "StaticVarCompensator.q")
            .or_else(|| flow.map(|value| value.1))
            .unwrap_or(0.0);
        svc.in_service = mapper.in_service(id);
        svc.uid = Some(id.to_string());
        values.push(svc);
    }
    Ok(values)
}

fn limit_kind<'a>(store: &'a Store, limit_type: &str, version: CgmesVersion) -> Option<&'a str> {
    match version {
        CgmesVersion::V2_4_15 => {
            store.enum_value(limit_type, "entsoe:OperationalLimitType.limitType")
        }
        CgmesVersion::V3_0 => store.enum_value(limit_type, "eu:OperationalLimitType.kind"),
    }
}

#[derive(Default)]
struct VoltageLimitCandidates {
    low_kv: Option<f64>,
    high_kv: Option<f64>,
    ids: Vec<String>,
}

fn voltage_limit_level<'a>(store: &'a Store, set: &str) -> Option<&'a str> {
    if let Some(terminal) = store.refv(set, "OperationalLimitSet.Terminal") {
        return terminal_voltage_level(store, terminal);
    }
    store
        .refv(set, "OperationalLimitSet.Equipment")
        .and_then(|equipment| equipment_voltage_level(store, equipment))
}

fn read_voltage_limit_candidates(
    store: &Store,
    version: CgmesVersion,
    warnings: &mut CgmesDiagnostics,
) -> HashMap<String, VoltageLimitCandidates> {
    let mut candidates: HashMap<String, VoltageLimitCandidates> = HashMap::new();
    for id in store.of_class("VoltageLimit") {
        let Some(set) = store.refv(id, "OperationalLimit.OperationalLimitSet") else {
            warnings.push_as(
                &codes::READ_CGMES_RECORD_UNMAPPED,
                format!("VoltageLimit `{id}` has no OperationalLimitSet and was not mapped"),
            );
            continue;
        };
        let Some(level) = voltage_limit_level(store, set) else {
            warnings.push_as(
                &codes::READ_CGMES_RECORD_UNMAPPED,
                format!(
                    "VoltageLimit `{id}` in OperationalLimitSet `{set}` does not target equipment or a terminal in one VoltageLevel and was not mapped"
                ),
            );
            continue;
        };
        let Some(limit_type) = store.refv(id, "OperationalLimit.OperationalLimitType") else {
            warnings.push_as(
                &codes::READ_CGMES_RECORD_UNMAPPED,
                format!("VoltageLimit `{id}` has no OperationalLimitType and was not mapped"),
            );
            continue;
        };
        let Some(value) = store
            .f(id, "VoltageLimit.normalValue")
            .or_else(|| store.f(id, "VoltageLimit.value"))
            .filter(|value| value.is_finite() && *value > 0.0)
        else {
            warnings.push_as(
                &codes::READ_CGMES_RECORD_UNMAPPED,
                format!(
                    "VoltageLimit `{id}` has no finite positive normalValue or value and was not mapped"
                ),
            );
            continue;
        };
        let entry = candidates.entry(level.to_string()).or_default();
        match limit_kind(store, limit_type, version) {
            Some(kind) if kind.eq_ignore_ascii_case("lowVoltage") => {
                entry.low_kv = Some(entry.low_kv.map_or(value, |current| current.max(value)));
            }
            Some(kind) if kind.eq_ignore_ascii_case("highVoltage") => {
                entry.high_kv = Some(entry.high_kv.map_or(value, |current| current.min(value)));
            }
            kind => {
                warnings.push_as(
                    &codes::READ_CGMES_RECORD_UNMAPPED,
                    format!(
                        "VoltageLimit `{id}` has OperationalLimitType kind `{}` instead of lowVoltage or highVoltage and was not mapped",
                        kind.unwrap_or("missing")
                    ),
                );
                continue;
            }
        }
        entry.ids.push(id.to_string());
    }
    candidates
}

fn valid_declared_voltage_limits(
    level: &VoltageLevel,
    warnings: &mut CgmesDiagnostics,
) -> (Option<f64>, Option<f64>) {
    let low = level.low_voltage_limit_kv;
    let high = level.high_voltage_limit_kv;
    if !low.zip(high).is_some_and(|(low, high)| low > high) {
        return (low, high);
    }
    warnings.push_as(
        &codes::READ_CGMES_VALUE_APPROXIMATED,
        format!(
            "VoltageLevel `{}` declares lowVoltageLimit {} kV above highVoltageLimit {} kV; both inconsistent VoltageLevel limits were ignored",
            level.component.local_id(),
            low.unwrap_or_default(),
            high.unwrap_or_default()
        ),
    );
    (None, None)
}

fn apply_voltage_limit_candidate(
    level: &mut VoltageLevel,
    candidate: &VoltageLimitCandidates,
    direct: (Option<f64>, Option<f64>),
    warnings: &mut CgmesDiagnostics,
) {
    let level_id = level.component.local_id();
    if candidate
        .low_kv
        .zip(candidate.high_kv)
        .is_some_and(|(low, high)| low > high)
    {
        warnings.push_as(
            &codes::READ_CGMES_RECORD_UNMAPPED,
            format!(
                "VoltageLimit records [{}] for VoltageLevel `{level_id}` form an inconsistent pair: low {} kV is above high {} kV; the pair was not mapped",
                candidate.ids.join(", "),
                candidate.low_kv.unwrap_or_default(),
                candidate.high_kv.unwrap_or_default()
            ),
        );
        level.low_voltage_limit_kv = direct.0;
        level.high_voltage_limit_kv = direct.1;
        return;
    }

    let combined_low = match (direct.0, candidate.low_kv) {
        (Some(direct), Some(limit)) => Some(direct.max(limit)),
        (direct, limit) => direct.or(limit),
    };
    let combined_high = match (direct.1, candidate.high_kv) {
        (Some(direct), Some(limit)) => Some(direct.min(limit)),
        (direct, limit) => direct.or(limit),
    };
    if combined_low
        .zip(combined_high)
        .is_some_and(|(low, high)| low > high)
    {
        warnings.push_as(
            &codes::READ_CGMES_RECORD_UNMAPPED,
            format!(
                "VoltageLimit records [{}] conflict with the valid VoltageLevel `{level_id}` range; the VoltageLimit records were not mapped",
                candidate.ids.join(", ")
            ),
        );
        level.low_voltage_limit_kv = direct.0;
        level.high_voltage_limit_kv = direct.1;
        return;
    }

    level.low_voltage_limit_kv = combined_low;
    level.high_voltage_limit_kv = combined_high;
    warnings.push_as(
        &codes::READ_CGMES_VALUE_APPROXIMATED,
        format!(
            "VoltageLimit records [{}] were combined with VoltageLevel `{level_id}` into its most restrictive valid lowVoltageLimit/highVoltageLimit pair; fresh CGMES emission writes the resulting VoltageLevel fields rather than the individual VoltageLimit records",
            candidate.ids.join(", ")
        ),
    );
}

fn apply_voltage_limits(
    store: &Store,
    version: CgmesVersion,
    voltage_levels: &mut [VoltageLevel],
    warnings: &mut CgmesDiagnostics,
) {
    let mut candidates = read_voltage_limit_candidates(store, version, warnings);
    for level in voltage_levels {
        let direct = valid_declared_voltage_limits(level, warnings);
        if let Some(candidate) = candidates.remove(level.component.local_id()) {
            apply_voltage_limit_candidate(level, &candidate, direct, warnings);
        } else {
            level.low_voltage_limit_kv = direct.0;
            level.high_voltage_limit_kv = direct.1;
        }
    }
}

fn loading_limits(
    store: &Store,
    set: &str,
    class: &str,
    version: CgmesVersion,
    warnings: &mut CgmesDiagnostics,
) -> Option<LoadingLimits> {
    let mut result = LoadingLimits::default();
    let mut found = false;
    for object in &store.objects {
        if object.class != class
            || store.refv(&object.id, "OperationalLimit.OperationalLimitSet") != Some(set)
        {
            continue;
        }
        let Some(limit_type) = store.refv(&object.id, "OperationalLimit.OperationalLimitType")
        else {
            warnings.push(format!(
                "{class} `{}` has no OperationalLimitType and was not retained",
                object.id
            ));
            continue;
        };
        let value = store
            .f(&object.id, &format!("{class}.normalValue"))
            .or_else(|| store.f(&object.id, &format!("{class}.value")));
        let Some(value) = value.filter(|value| value.is_finite() && *value > 0.0) else {
            warnings.push(format!(
                "{class} `{}` has a missing, nonfinite, or nonpositive value and was not retained",
                object.id
            ));
            continue;
        };
        found = true;
        let kind = limit_kind(store, limit_type, version);
        let permanent = store
            .boolean(limit_type, "OperationalLimitType.isInfiniteDuration")
            .unwrap_or_else(|| kind == Some("patl"));
        let name = store.name(&object.id);
        if let Some(kind) = kind {
            let canonical_kind = if permanent { "patl" } else { "tatl" };
            if !kind.eq_ignore_ascii_case(canonical_kind) {
                warnings.push_as(
                    &codes::READ_CGMES_VALUE_APPROXIMATED,
                    format!(
                        "{class} `{}` uses OperationalLimitType kind `{kind}`; PowerIO retains it as a {} limit and fresh CGMES emits kind `{canonical_kind}`",
                        object.id,
                        if permanent { "permanent" } else { "temporary" }
                    ),
                );
            }
        }
        if permanent {
            if result.permanent_limit.is_some() {
                warnings.push_as(&codes::READ_CGMES_VALUE_APPROXIMATED, format!(
                    "OperationalLimitSet `{set}` contains several permanent {class} values; the smallest is retained"
                ));
            }
            if result.permanent_limit.is_none_or(|current| value < current) {
                result.permanent_limit = Some(value);
                result.permanent_limit_name = Some(name);
            }
        } else {
            let duration = match store.f(limit_type, "OperationalLimitType.acceptableDuration") {
                None => 0,
                Some(duration_value)
                    if !duration_value.is_finite()
                        || duration_value < 0.0
                        || duration_value.round() >= u64::MAX as f64 =>
                {
                    warnings.push_as(
                        &codes::READ_CGMES_VALUE_APPROXIMATED,
                        format!(
                            "{class} `{}` uses OperationalLimitType.acceptableDuration={duration_value} seconds, which cannot be represented as a nonnegative whole-second duration; PowerIO retains the limit with 0 seconds and fresh CGMES emits 0",
                            object.id
                        ),
                    );
                    0
                }
                Some(duration_value) => {
                    let duration = duration_value.round() as u64;
                    if duration_value.fract() != 0.0 {
                        warnings.push_as(
                            &codes::READ_CGMES_VALUE_APPROXIMATED,
                            format!(
                                "{class} `{}` uses OperationalLimitType.acceptableDuration={duration_value} seconds; PowerIO rounds it to {duration} whole seconds and fresh CGMES emits the rounded value",
                                object.id
                            ),
                        );
                    }
                    duration
                }
            };
            result.temporary_limits.push(TemporaryLimit {
                name,
                value,
                acceptable_duration_seconds: duration,
                fictitious: store
                    .boolean(&object.id, "IdentifiedObject.isFictitious")
                    .unwrap_or(false),
            });
        }
    }
    found.then_some(result)
}

fn read_operational_limit_groups(
    mapper: &mut Mapper<'_>,
    version: CgmesVersion,
) -> Result<Vec<OperationalLimitGroup>> {
    let store = mapper.store;
    let mut groups = Vec::new();
    let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
    for set in store.of_class("OperationalLimitSet") {
        let targets: Vec<(String, u8)> =
            if let Some(terminal) = store.refv(set, "OperationalLimitSet.Terminal") {
                terminal_reference(store, terminal)?
                    .map(|value| vec![(value.equipment.local_id().to_string(), value.terminal)])
                    .unwrap_or_default()
            } else if let Some(equipment) = store.refv(set, "OperationalLimitSet.Equipment") {
                let terminals = mapper.wiring.terminals(equipment);
                if terminals.is_empty() {
                    vec![(equipment.to_string(), 1)]
                } else {
                    terminals
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            (
                                equipment.to_string(),
                                u8::try_from(index + 1).unwrap_or(u8::MAX),
                            )
                        })
                        .collect()
                }
            } else {
                warnings.push(format!(
                    "OperationalLimitSet `{set}` has neither a Terminal nor Equipment target"
                ));
                continue;
            };
        let current_limits = loading_limits(store, set, "CurrentLimit", version, &mut warnings);
        let active_power_limits =
            loading_limits(store, set, "ActivePowerLimit", version, &mut warnings);
        let apparent_power_limits =
            loading_limits(store, set, "ApparentPowerLimit", version, &mut warnings);
        if current_limits.is_none()
            && active_power_limits.is_none()
            && apparent_power_limits.is_none()
        {
            continue;
        }
        for (equipment, terminal) in targets {
            let class = store.class_of(&equipment).unwrap_or_default();
            groups.push(OperationalLimitGroup {
                equipment: component_id(component_type(class), &equipment)?,
                terminal,
                id: set.to_string(),
                properties: BTreeMap::new(),
                selected: false,
                current_limits: current_limits.clone(),
                active_power_limits: active_power_limits.clone(),
                apparent_power_limits: apparent_power_limits.clone(),
            });
        }
    }
    mapper.warnings.extend(warnings);
    Ok(groups)
}

fn tap_control_mode(
    store: &Store,
    tap: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<TapChangerRegulationMode> {
    let control = store.refv(tap, "TapChanger.TapChangerControl")?;
    match store.enum_value(control, "RegulatingControl.mode")? {
        "voltage" => Some(TapChangerRegulationMode::Voltage),
        "reactivePower" => Some(TapChangerRegulationMode::ReactivePower),
        "activePower" => Some(TapChangerRegulationMode::ActivePower),
        "currentFlow" => Some(TapChangerRegulationMode::Current),
        mode => {
            warnings.push_as(
                &codes::READ_CGMES_VALUE_APPROXIMATED,
                format!(
                    "tap changer `{tap}` RegulatingControl.mode `{mode}` has no PowerIO tap regulation mode; the typed control has no mode and fresh CGMES output selects the default for the tap changer kind"
                ),
            );
            None
        }
    }
}

fn table_tap_steps(store: &Store, tap: &str, kind: TapChangerKind) -> Option<Vec<TapChangerStep>> {
    let (table_property, point_class, point_table_property) = match kind {
        TapChangerKind::Ratio => (
            "RatioTapChanger.RatioTapChangerTable",
            "RatioTapChangerTablePoint",
            "RatioTapChangerTablePoint.RatioTapChangerTable",
        ),
        TapChangerKind::Phase => (
            "PhaseTapChangerTabular.PhaseTapChangerTable",
            "PhaseTapChangerTablePoint",
            "PhaseTapChangerTablePoint.PhaseTapChangerTable",
        ),
    };
    let table = store.refv(tap, table_property)?;
    let mut steps = store
        .of_class(point_class)
        .filter(|point| store.refv(point, point_table_property) == Some(table))
        .map(|point| TapChangerStep {
            position: store
                .f(point, "TapChangerTablePoint.step")
                .unwrap_or(0.0)
                .round() as i32,
            rho: store.f(point, "TapChangerTablePoint.ratio").unwrap_or(1.0),
            alpha_degrees: store
                .f(point, "PhaseTapChangerTablePoint.angle")
                .unwrap_or(0.0),
            resistance_deviation_percent: store.f(point, "TapChangerTablePoint.r").unwrap_or(0.0),
            reactance_deviation_percent: store.f(point, "TapChangerTablePoint.x").unwrap_or(0.0),
            conductance_deviation_percent: store.f(point, "TapChangerTablePoint.g").unwrap_or(0.0),
            susceptance_deviation_percent: store.f(point, "TapChangerTablePoint.b").unwrap_or(0.0),
        })
        .collect::<Vec<_>>();
    steps.sort_by_key(|step| step.position);
    (!steps.is_empty()).then_some(steps)
}

fn apply_phase_tap_reactance_deviations(
    store: &Store,
    tap: &str,
    class: &str,
    steps: &mut [TapChangerStep],
) {
    let Some(end) = store.refv(tap, "PhaseTapChanger.TransformerEnd") else {
        return;
    };
    let nominal_x = store.f(end, "PowerTransformerEnd.x").unwrap_or(0.0);
    if nominal_x == 0.0 {
        return;
    }
    let x_min = store
        .f(tap, "PhaseTapChanger.xStepMin")
        .or_else(|| store.f(tap, "PhaseTapChangerLinear.xMin"))
        .or_else(|| store.f(tap, "PhaseTapChangerNonLinear.xMin"))
        .filter(|value| *value >= 0.0)
        .unwrap_or(nominal_x);
    let Some(x_max) = store
        .f(tap, "PhaseTapChanger.xStepMax")
        .or_else(|| store.f(tap, "PhaseTapChangerLinear.xMax"))
        .or_else(|| store.f(tap, "PhaseTapChangerNonLinear.xMax"))
    else {
        return;
    };
    if x_min < 0.0 || x_max <= 0.0 || x_min > x_max {
        return;
    }
    let alpha_max = steps
        .iter()
        .map(|step| step.alpha_degrees)
        .reduce(f64::max)
        .unwrap_or(0.0);
    if alpha_max == 0.0 {
        return;
    }
    let alpha_max_radians = alpha_max.to_radians();
    let winding_angle = store
        .f(tap, "PhaseTapChangerAsymmetrical.windingConnectionAngle")
        .unwrap_or(90.0)
        .to_radians();
    for step in steps {
        let alpha = step.alpha_degrees.to_radians();
        let x = if class == "PhaseTapChangerAsymmetrical" {
            let numerator = winding_angle.sin() - alpha_max_radians.tan() * winding_angle.cos();
            let denominator = winding_angle.sin() - alpha.tan() * winding_angle.cos();
            let factor = alpha.tan() / alpha_max_radians.tan() * numerator / denominator;
            x_min + (x_max - x_min) * factor.powi(2)
        } else {
            let factor = (alpha / 2.0).sin() / (alpha_max_radians / 2.0).sin();
            x_min + (x_max - x_min) * factor.powi(2)
        };
        step.reactance_deviation_percent = 100.0 * (x - nominal_x) / nominal_x;
    }
}

fn calculated_tap_steps(
    store: &Store,
    tap: &str,
    kind: TapChangerKind,
) -> Result<Vec<TapChangerStep>> {
    let low = store.f(tap, "TapChanger.lowStep").unwrap_or(0.0).round() as i32;
    let high = store
        .f(tap, "TapChanger.highStep")
        .unwrap_or(f64::from(low))
        .round() as i32;
    if high < low || i64::from(high) - i64::from(low) > 10_000 {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!("tap changer `{tap}` has an invalid low/high step range"),
        });
    }
    let neutral = store
        .f(tap, "TapChanger.neutralStep")
        .unwrap_or(f64::from(low))
        .round() as i32;
    let class = store.class_of(tap).unwrap_or_default();
    let voltage_increment = store
        .f(tap, "RatioTapChanger.stepVoltageIncrement")
        .or_else(|| store.f(tap, "PhaseTapChangerNonLinear.voltageStepIncrement"))
        .unwrap_or(0.0);
    let phase_increment = store
        .f(tap, "PhaseTapChangerLinear.stepPhaseShiftIncrement")
        .or_else(|| store.f(tap, "PhaseTapChangerSymmetrical.stepPhaseShiftIncrement"));
    let winding_angle = store
        .f(tap, "PhaseTapChangerAsymmetrical.windingConnectionAngle")
        .unwrap_or(90.0)
        .to_radians();
    let mut steps = (low..=high)
        .map(|position| {
            let offset = f64::from(position - neutral);
            let (rho, alpha_degrees) = match (kind, class) {
                (TapChangerKind::Ratio, _) => (1.0 + offset * voltage_increment / 100.0, 0.0),
                (TapChangerKind::Phase, "PhaseTapChangerAsymmetrical") => {
                    let increment = offset * voltage_increment / 100.0;
                    let dx = 1.0 + increment * winding_angle.cos();
                    let dy = increment * winding_angle.sin();
                    (dx.hypot(dy), dy.atan2(dx).to_degrees())
                }
                (TapChangerKind::Phase, "PhaseTapChangerSymmetrical") => {
                    let alpha = phase_increment.map_or_else(
                        || 2.0 * (offset * voltage_increment / 200.0).atan().to_degrees(),
                        |increment| offset * increment,
                    );
                    (1.0, alpha)
                }
                (TapChangerKind::Phase, _) => (1.0, offset * phase_increment.unwrap_or(0.0)),
            };
            TapChangerStep {
                position,
                rho,
                alpha_degrees,
                resistance_deviation_percent: 0.0,
                reactance_deviation_percent: 0.0,
                conductance_deviation_percent: 0.0,
                susceptance_deviation_percent: 0.0,
            }
        })
        .collect::<Vec<_>>();
    if kind == TapChangerKind::Phase {
        apply_phase_tap_reactance_deviations(store, tap, class, &mut steps);
    }
    Ok(steps)
}

fn read_tap_changers(mapper: &mut Mapper<'_>) -> Result<Vec<TapChanger>> {
    let store = mapper.store;
    let mut result = Vec::new();
    for end in store.of_class("PowerTransformerEnd") {
        let Some(transformer) = store.refv(end, "PowerTransformerEnd.PowerTransformer") else {
            continue;
        };
        let winding = store
            .f(end, "TransformerEnd.endNumber")
            .unwrap_or(1.0)
            .round() as u8;
        let mut taps = Vec::new();
        if let Some(tap) = ratio_tap_changer(store, end) {
            taps.push((tap.to_string(), TapChangerKind::Ratio));
        }
        if let Some(tap) = phase_tap_changer(store, end) {
            taps.push((tap, TapChangerKind::Phase));
        }
        for (tap, kind) in taps {
            let low = store.f(&tap, "TapChanger.lowStep").unwrap_or(0.0).round() as i32;
            let neutral_tap_position = store
                .f(&tap, "TapChanger.neutralStep")
                .map(|value| value.round() as i32);
            let normal_tap_position = store
                .f(&tap, "TapChanger.normalStep")
                .map(|value| value.round() as i32);
            let control = store.refv(&tap, "TapChanger.TapChangerControl");
            let steps = table_tap_steps(store, &tap, kind)
                .map_or_else(|| calculated_tap_steps(store, &tap, kind), Ok)?;
            let tap_position = store
                .f(&tap, "TapChanger.step")
                .or_else(|| store.f(&tap, "TapChanger.normalStep"))
                .or_else(|| store.f(&tap, "TapChanger.neutralStep"))
                .unwrap_or(f64::from(low))
                .round() as i32;
            let solved_tap_position = sv_tap_step(store, &tap).map(|value| value.round() as i32);
            result.push(TapChanger {
                component: Some(component_id("tap_changer", &tap)?),
                transformer: component_id("branch", transformer)?,
                winding,
                kind,
                tap_position: Some(tap_position),
                solved_tap_position,
                low_tap_position: low,
                neutral_tap_position,
                normal_tap_position,
                voltage_step_increment_percent: store
                    .f(&tap, "RatioTapChanger.stepVoltageIncrement")
                    .or_else(|| store.f(&tap, "PhaseTapChangerNonLinear.voltageStepIncrement")),
                load_tap_changing_capabilities: store
                    .boolean(&tap, "TapChanger.ltcFlag")
                    .unwrap_or(false),
                regulating: store
                    .boolean(&tap, "TapChanger.controlEnabled")
                    .or_else(|| {
                        control.and_then(|value| store.boolean(value, "RegulatingControl.enabled"))
                    })
                    .unwrap_or(false),
                regulation_mode: tap_control_mode(store, &tap, mapper.warnings),
                regulation_value: control
                    .and_then(|value| store.f(value, "RegulatingControl.targetValue")),
                target_deadband: control
                    .and_then(|value| store.f(value, "RegulatingControl.targetDeadband")),
                regulation_terminal: control
                    .and_then(|value| store.refv(value, "RegulatingControl.Terminal"))
                    .map(|terminal| terminal_reference(store, terminal))
                    .transpose()?
                    .flatten(),
                steps,
            });
        }
    }
    Ok(result)
}

fn normalized_load_coefficients(
    values: [f64; 3],
    quantity: &str,
    response: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<[f64; 3]> {
    if !values.iter().all(|value| value.is_finite()) {
        warnings.push(format!(
            "LoadResponseCharacteristic `{response}` has a nonfinite {quantity} coefficient and was not applied"
        ));
        return None;
    }
    let sum: f64 = values.iter().sum();
    if sum.abs() <= f64::EPSILON {
        warnings.push(format!(
            "LoadResponseCharacteristic `{response}` has {quantity} coefficients that sum to zero and was not applied"
        ));
        return None;
    }
    if (sum - 1.0).abs() > 1e-9 {
        warnings.push_as(&codes::READ_CGMES_VALUE_APPROXIMATED, format!(
            "LoadResponseCharacteristic `{response}` has {quantity} coefficients that sum to {sum}; they were normalized to one"
        ));
    }
    Some(values.map(|value| value / sum))
}

fn read_load_voltage_model(
    mapper: &mut Mapper<'_>,
    load: &str,
    p: f64,
    q: f64,
) -> Option<LoadVoltageModel> {
    let store = mapper.store;
    let response = store.refv(load, "EnergyConsumer.LoadResponse")?;
    if store
        .boolean(response, "LoadResponseCharacteristic.exponentModel")
        .unwrap_or(false)
    {
        let gamma_p = store
            .f(response, "LoadResponseCharacteristic.pVoltageExponent")
            .unwrap_or(0.0);
        let gamma_q = store
            .f(response, "LoadResponseCharacteristic.qVoltageExponent")
            .unwrap_or(0.0);
        return if gamma_p == 0.0 && gamma_q == 0.0 {
            Some(LoadVoltageModel::ConstantPower)
        } else {
            Some(LoadVoltageModel::Exponential {
                p,
                q,
                v_nom: None,
                gamma_p,
                gamma_q,
            })
        };
    }

    let coefficient = |name: &str| store.f(response, &format!("LoadResponseCharacteristic.{name}"));
    let p_coefficients = normalized_load_coefficients(
        [
            coefficient("pConstantPower")?,
            coefficient("pConstantCurrent")?,
            coefficient("pConstantImpedance")?,
        ],
        "active power",
        response,
        mapper.warnings,
    )?;
    let q_coefficients = normalized_load_coefficients(
        [
            coefficient("qConstantPower")?,
            coefficient("qConstantCurrent")?,
            coefficient("qConstantImpedance")?,
        ],
        "reactive power",
        response,
        mapper.warnings,
    )?;
    let is_constant_power = |coefficients: [f64; 3]| {
        (coefficients[0] - 1.0).abs() <= 1e-12
            && coefficients[1].abs() <= 1e-12
            && coefficients[2].abs() <= 1e-12
    };
    if is_constant_power(p_coefficients) && is_constant_power(q_coefficients) {
        Some(LoadVoltageModel::ConstantPower)
    } else {
        Some(LoadVoltageModel::Zip {
            p_constant_power: p * p_coefficients[0],
            q_constant_power: q * q_coefficients[0],
            p_constant_current: p * p_coefficients[1],
            q_constant_current: q * q_coefficients[1],
            p_constant_impedance: p * p_coefficients[2],
            q_constant_impedance: q * q_coefficients[2],
            v_nom: None,
            load_type: None,
            scaling: None,
        })
    }
}

fn read_loads(mapper: &mut Mapper<'_>) -> Vec<Load> {
    let mut loads = Vec::new();
    for class in LOAD_CLASSES {
        for id in mapper.store.of_class(class) {
            let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
                mapper.warnings.push(format!(
                    "{class} {}: no terminal on a topological node; skipped",
                    mapper.store.name(id)
                ));
                continue;
            };
            let (p, q) = mapper.power(id, "EnergyConsumer");
            let mut load = Load::new(bus, p, q);
            load.voltage_model = read_load_voltage_model(mapper, id, p, q);
            load.in_service = mapper.in_service(id);
            load.uid = Some(id.to_string());
            loads.push(load);
        }
    }
    loads
}

fn generating_unit_active_power_control(
    store: &Store,
    unit: &str,
) -> Result<Option<ActivePowerControl>> {
    let Some(text) = store.text(unit, "GeneratingUnit.normalPF") else {
        return Ok(None);
    };
    let participation_factor = text.trim().parse::<f64>().map_err(|_| Error::FormatRead {
        format: FMT,
        message: format!(
            "GeneratingUnit {} has a nonnumeric normalPF `{text}`",
            store.name(unit)
        ),
    })?;
    if !participation_factor.is_finite() || participation_factor < 0.0 {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!(
                "GeneratingUnit {} has invalid normalPF `{text}`; expected a finite nonnegative distributed slack participation factor",
                store.name(unit)
            ),
        });
    }
    let mut control = ActivePowerControl::new(true);
    control.participation_factor = Some(participation_factor);
    Ok(Some(control))
}

fn generating_unit_energy_source(store: &Store, unit: &str) -> GeneratorEnergySource {
    match store.class_of(unit) {
        Some("HydroGeneratingUnit") => GeneratorEnergySource::Hydro,
        Some("NuclearGeneratingUnit") => GeneratorEnergySource::Nuclear,
        Some("WindGeneratingUnit") => GeneratorEnergySource::Wind,
        Some("ThermalGeneratingUnit") => GeneratorEnergySource::Thermal,
        Some("SolarGeneratingUnit") => GeneratorEnergySource::Solar,
        _ => GeneratorEnergySource::Other,
    }
}

fn read_machines(
    mapper: &mut Mapper<'_>,
) -> Result<(Vec<Generator>, Option<BusId>, Vec<EquipmentReactiveLimits>)> {
    let mut generators = Vec::new();
    let mut equipment_reactive_limits = Vec::new();
    let mut best: Option<(f64, BusId)> = None;
    let mut external: Option<BusId> = None;
    let mut largest: Option<(f64, BusId)> = None;
    for id in mapper.store.of_class("SynchronousMachine") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            mapper.warnings.push(format!(
                "SynchronousMachine {}: no terminal on a topological node; skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let store = mapper.store;
        let (p, q) = mapper.power(id, "RotatingMachine");
        let mut generator = Generator::new(bus);
        generator.pg = -p;
        generator.qg = -q;
        generator.qmax = store.f(id, "SynchronousMachine.maxQ").unwrap_or(0.0);
        generator.qmin = store.f(id, "SynchronousMachine.minQ").unwrap_or(0.0);
        if let Some(limits) = synchronous_machine_reactive_limits(store, id, mapper.warnings) {
            let (minimum, maximum) = crate::network::calc_reactive_limits_at_active_power(
                &format!("SynchronousMachine {}", store.name(id)),
                &limits,
                generator.pg,
            )
            .map_err(|message| Error::FormatRead {
                format: FMT,
                message,
            })?;
            generator.qmin = minimum;
            generator.qmax = maximum;
            equipment_reactive_limits.push(EquipmentReactiveLimits {
                equipment: component_id("generator", id)?,
                limits,
            });
        }
        generator.mbase = store.f(id, "RotatingMachine.ratedS").unwrap_or(0.0);
        generator.in_service = mapper.in_service(id);
        generator.uid = Some(id.to_string());
        if let Some(unit) = store.refv(id, "RotatingMachine.GeneratingUnit") {
            generator.energy_source = generating_unit_energy_source(store, unit);
            generator.pmin = store.f(unit, "GeneratingUnit.minOperatingP").unwrap_or(0.0);
            generator.pmax = store.f(unit, "GeneratingUnit.maxOperatingP").unwrap_or(0.0);
            generator.active_power_control = generating_unit_active_power_control(store, unit)?;
        }
        apply_regulation(mapper, id, &mut generator)?;
        if let Some(priority) = mapper.store.f(id, "SynchronousMachine.referencePriority") {
            if priority > 0.0 && best.is_none_or(|(p0, _)| priority < p0) {
                best = Some((priority, bus));
            }
        }
        if largest.is_none_or(|(s, _)| generator.mbase > s) {
            largest = Some((generator.mbase, bus));
        }
        generators.push(generator);
    }
    for id in mapper.store.of_class("ExternalNetworkInjection") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let store = mapper.store;
        let (p, q) = mapper.power(id, "ExternalNetworkInjection");
        let mut generator = Generator::new(bus);
        mapper.warnings.push_as(
            &codes::READ_CGMES_VALUE_APPROXIMATED,
            format!(
                "ExternalNetworkInjection `{id}` is represented as a balanced generator; fresh CGMES output emits a SynchronousMachine because the generator table does not retain the external-network voltage characteristic fields"
            ),
        );
        generator.pg = -p;
        generator.qg = -q;
        generator.pmax = store.f(id, "ExternalNetworkInjection.maxP").unwrap_or(0.0);
        generator.pmin = store.f(id, "ExternalNetworkInjection.minP").unwrap_or(0.0);
        generator.qmax = store.f(id, "ExternalNetworkInjection.maxQ").unwrap_or(0.0);
        generator.qmin = store.f(id, "ExternalNetworkInjection.minQ").unwrap_or(0.0);
        generator.in_service = mapper.in_service(id);
        generator.uid = Some(id.to_string());
        apply_regulation(mapper, id, &mut generator)?;
        if external.is_none() {
            external = Some(bus);
        }
        generators.push(generator);
    }
    let reference = best
        .map(|(_, bus)| bus)
        .or(external)
        .or(largest.map(|(_, b)| b));
    Ok((generators, reference, equipment_reactive_limits))
}

/// Voltage-mode `RegulatingControl` → `vg` (target over the regulated node's
/// base) and the remote regulated bus when it is not the machine's own.
fn apply_regulation(
    mapper: &mut Mapper<'_>,
    machine: &str,
    generator: &mut Generator,
) -> Result<()> {
    let store = mapper.store;
    generator.voltage_regulation_on = false;
    let Some(control) = store.refv(machine, "RegulatingCondEq.RegulatingControl") else {
        return Ok(());
    };
    if mapper.store.enum_value(control, "RegulatingControl.mode") != Some("voltage") {
        if let Some(mode) = mapper.store.enum_value(control, "RegulatingControl.mode") {
            mapper.warnings.push(format!(
                "SynchronousMachine {} uses RegulatingControl.mode `{mode}`; the exact control is retained for CGMES emission but Generator voltage regulation fields do not model it",
                store.name(machine)
            ));
        }
        return Ok(());
    }
    generator.voltage_regulation_on = store
        .boolean(control, "RegulatingControl.enabled")
        .unwrap_or(false)
        && store
            .boolean(machine, "RegulatingCondEq.controlEnabled")
            .unwrap_or(true);
    let terminal = store.refv(control, "RegulatingControl.Terminal");
    generator.regulating_terminal = terminal
        .map(|terminal| terminal_reference(store, terminal))
        .transpose()?
        .flatten();
    let regulated = terminal
        .and_then(|t| mapper.wiring.node(t))
        .and_then(|tn| mapper.bus_of_node.get(tn))
        .copied();
    if let Some(bus) = regulated {
        if let Some(target) = regulating_control_target_kv(store, control, mapper.warnings)
            && target > 0.0
        {
            generator.vg = target / mapper.kv(bus);
        }
        if bus != generator.bus {
            generator.regulated_bus = Some(bus);
        }
    }
    Ok(())
}

fn regulating_control_target_kv(
    store: &Store,
    control: &str,
    warnings: &mut CgmesDiagnostics,
) -> Option<f64> {
    let target = store.f(control, "RegulatingControl.targetValue")?;
    Some(target * regulating_control_scale_to_kv(store, control, warnings))
}

fn regulating_control_scale_to_kv(
    store: &Store,
    control: &str,
    warnings: &mut CgmesDiagnostics,
) -> f64 {
    let multiplier = store
        .enum_value(control, "RegulatingControl.targetValueUnitMultiplier")
        .unwrap_or("k");
    match multiplier {
        "none" => 1e-3,
        "m" => 1e-6,
        "k" => 1.0,
        "M" => 1e3,
        "G" => 1e6,
        other => {
            warnings.push_as(&codes::READ_CGMES_VALUE_DEFAULTED, format!(
                "RegulatingControl {} has unsupported targetValueUnitMultiplier `{other}`; targetValue is interpreted as kV",
                store.name(control)
            ));
            1.0
        }
    }
}

fn read_shunts(mapper: &mut Mapper<'_>) -> Result<Vec<Shunt>> {
    let mut shunts = read_linear_shunts(mapper)?;
    shunts.extend(read_nonlinear_shunts(mapper)?);
    Ok(shunts)
}

fn read_linear_shunts(mapper: &mut Mapper<'_>) -> Result<Vec<Shunt>> {
    let mut shunts = Vec::new();
    for id in mapper.store.of_class("LinearShuntCompensator") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let store = mapper.store;
        let kv = mapper.kv(bus);
        let sections = selected_sections(mapper, id, 1.0);
        let maximum_sections = store
            .f(id, "ShuntCompensator.maximumSections")
            .unwrap_or(sections.max(1) as f64)
            .max(sections as f64)
            .round() as usize;
        // Siemens × kV² = MW/MVAr injected at nominal voltage, the model's
        // 1 p.u. convention.
        let g_per_section = store
            .f(id, "LinearShuntCompensator.gPerSection")
            .unwrap_or(0.0)
            * (kv * kv);
        let b_per_section = store
            .f(id, "LinearShuntCompensator.bPerSection")
            .unwrap_or(0.0)
            * (kv * kv);
        let blocks = vec![ShuntBlock::with_admittance(
            maximum_sections.min(u32::MAX as usize) as u32,
            g_per_section,
            b_per_section,
        )];
        let mut shunt = Shunt::new(
            bus,
            g_per_section * sections as f64,
            b_per_section * sections as f64,
        );
        shunt.section_count = Some(sections.min(u32::MAX as usize) as u32);
        shunt.in_service = mapper.in_service(id);
        shunt.control = shunt_control(mapper, id, blocks, maximum_sections > 1)?;
        shunt.uid = Some(id.to_string());
        shunts.push(shunt);
    }
    Ok(shunts)
}

fn read_nonlinear_shunts(mapper: &mut Mapper<'_>) -> Result<Vec<Shunt>> {
    let mut shunts = Vec::new();
    for id in mapper.store.of_class("NonlinearShuntCompensator") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let store = mapper.store;
        let kv2 = mapper.kv(bus).powi(2);
        let sections = selected_sections(mapper, id, 0.0);
        let mut points: Vec<_> = store
            .of_class("NonlinearShuntCompensatorPoint")
            .filter(|point| {
                store.refv(
                    point,
                    "NonlinearShuntCompensatorPoint.NonlinearShuntCompensator",
                ) == Some(id)
            })
            .filter_map(|point| {
                let number = store
                    .f(point, "NonlinearShuntCompensatorPoint.sectionNumber")?
                    .round() as usize;
                (number > 0).then_some((
                    number,
                    store
                        .f(point, "NonlinearShuntCompensatorPoint.g")
                        .unwrap_or(0.0)
                        * kv2,
                    store
                        .f(point, "NonlinearShuntCompensatorPoint.b")
                        .unwrap_or(0.0)
                        * kv2,
                ))
            })
            .collect();
        points.sort_by_key(|point| point.0);
        let maximum_sections = store
            .f(id, "ShuntCompensator.maximumSections")
            .unwrap_or(points.len() as f64)
            .max(sections as f64)
            .round() as usize;
        if points.len() != maximum_sections
            || points
                .iter()
                .enumerate()
                .any(|(index, point)| point.0 != index + 1)
        {
            mapper.warnings.push(format!(
                "NonlinearShuntCompensator {id}: section point numbers do not cover 1 through {maximum_sections}"
            ));
        }
        let blocks: Vec<_> = points
            .iter()
            .map(|(_, g, b)| ShuntBlock::with_admittance(1, *g, *b))
            .collect();
        let active = sections.min(blocks.len());
        let g = blocks.iter().take(active).map(|block| block.g).sum();
        let b = blocks.iter().take(active).map(|block| block.b).sum();
        let mut shunt = Shunt::new(bus, g, b);
        shunt.section_count = Some(active.min(u32::MAX as usize) as u32);
        shunt.in_service = mapper.in_service(id);
        shunt.control = shunt_control(mapper, id, blocks, true)?;
        shunt.uid = Some(id.to_string());
        shunts.push(shunt);
    }
    Ok(shunts)
}

fn shunt_control(
    mapper: &mut Mapper<'_>,
    shunt: &str,
    blocks: Vec<ShuntBlock>,
    keep_without_regulation: bool,
) -> Result<Option<SwitchedShuntControl>> {
    let store = mapper.store;
    let regulation = store.refv(shunt, "RegulatingCondEq.RegulatingControl");
    if regulation.is_none() && !keep_without_regulation {
        return Ok(None);
    }
    let equipment_enabled = store
        .boolean(shunt, "RegulatingCondEq.controlEnabled")
        .unwrap_or(false);
    let control_enabled = regulation
        .and_then(|control| store.boolean(control, "RegulatingControl.enabled"))
        .unwrap_or(equipment_enabled);
    let enabled = equipment_enabled && control_enabled;
    let regulating_terminal = regulation
        .and_then(|control| store.refv(control, "RegulatingControl.Terminal"))
        .map(|terminal| terminal_reference(store, terminal))
        .transpose()?
        .flatten();
    let regulated_bus = regulation
        .and_then(|control| store.refv(control, "RegulatingControl.Terminal"))
        .and_then(|terminal| mapper.wiring.node(terminal))
        .and_then(|node| mapper.bus_of_node.get(node))
        .copied();
    let regulated_kv = regulated_bus.map_or(1.0, |bus| mapper.kv(bus));
    let scale_to_kv = regulation.map_or(1.0, |control| {
        regulating_control_scale_to_kv(store, control, mapper.warnings)
    });
    let target = regulation
        .and_then(|control| store.f(control, "RegulatingControl.targetValue"))
        .unwrap_or(0.0)
        * scale_to_kv;
    let deadband = regulation
        .and_then(|control| store.f(control, "RegulatingControl.targetDeadband"))
        .unwrap_or(0.0)
        * scale_to_kv;
    Ok(Some(SwitchedShuntControl {
        mode: if enabled {
            SwitchedShuntMode::Discrete
        } else {
            SwitchedShuntMode::Locked
        },
        vhigh: (target + deadband / 2.0) / regulated_kv,
        vlow: (target - deadband / 2.0) / regulated_kv,
        control_bus: regulated_bus,
        regulating_terminal,
        rmpct: 100.0,
        blocks,
    }))
}

/// The shunt's selected section count: the SSH `ShuntCompensator.sections`
/// assignment, else the SV `SvShuntCompensatorSections` observation, else
/// `ShuntCompensator.normalSections`, else `default`. One source neutral
/// record holds one count, so an SV observation that differs from the SSH
/// assignment is reported and not retained.
fn selected_sections(mapper: &mut Mapper<'_>, id: &str, default: f64) -> usize {
    let store = mapper.store;
    let assigned = store.f(id, "ShuntCompensator.sections");
    let observed = sv_sections(store, id);
    if let (Some(assigned), Some(observed)) = (assigned, observed) {
        if (assigned - observed).abs() > f64::EPSILON {
            mapper.warnings.push_as(
                &codes::READ_CGMES_FIELD_UNMAPPED,
                format!(
                    "`SvShuntCompensatorSections.sections` for `{id}` is {observed} while the SSH `ShuntCompensator.sections` assignment is {assigned}; the shunt keeps the SSH assignment and the state variable count is not retained"
                ),
            );
        }
    }
    assigned
        .or(observed)
        .or_else(|| store.f(id, "ShuntCompensator.normalSections"))
        .unwrap_or(default)
        .max(0.0)
        .round() as usize
}

fn sv_sections(store: &Store, shunt: &str) -> Option<f64> {
    store.of_class("SvShuntCompensatorSections").find_map(|sv| {
        (store.refv(sv, "SvShuntCompensatorSections.ShuntCompensator") == Some(shunt))
            .then(|| store.f(sv, "SvShuntCompensatorSections.sections"))
            .flatten()
    })
}

fn read_switches(mapper: &mut Mapper<'_>) -> Vec<Switch> {
    let mut switches = Vec::new();
    let mut internal = 0usize;
    for class in SWITCH_CLASSES {
        for id in mapper.store.of_class(class) {
            let (Some(from), Some(to)) = (
                mapper.bus_of_equipment_terminal(id, 0),
                mapper.bus_of_equipment_terminal(id, 1),
            ) else {
                continue;
            };
            if from == to {
                // Closed inside one bus: the topology processor that produced
                // TP, or the calculated topology, already joined its ends.
                internal += 1;
                continue;
            }
            let store = mapper.store;
            let open = switch_is_open(store, id);
            let mut switch = Switch::new(from, to, !open);
            switch.current_rating = store.f(id, "Switch.ratedCurrent");
            switch.uid = Some(id.to_string());
            switches.push(switch);
        }
    }
    if internal > 0 {
        let bus_kind = match mapper.topology {
            BusSource::TopologicalNodes => "topological node",
            BusSource::Calculated => "calculated bus",
        };
        mapper.warnings.push_as(
            &codes::READ_CGMES_VALUE_APPROXIMATED,
            format!(
                "{internal} switch(es) internal to one {bus_kind} are represented \
             by the topology itself"
            ),
        );
    }
    switches
}

/// Boundary-point injections become loads: they model the neighboring
/// network's net demand at the tie node.
fn read_equivalent_injections(
    mapper: &mut Mapper<'_>,
    loads: &mut Vec<Load>,
    generators: &mut Vec<Generator>,
) {
    let mut count = 0usize;
    for id in mapper.store.of_class("EquivalentInjection") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let (p, q) = mapper.power(id, "EquivalentInjection");
        let regulation = mapper
            .store
            .boolean(id, "EquivalentInjection.regulationStatus")
            .unwrap_or(false);
        if regulation {
            let mut generator = Generator::new(bus);
            generator.pg = -p;
            generator.qg = -q;
            generator.in_service = mapper.in_service(id);
            generator.uid = Some(id.to_string());
            generators.push(generator);
        } else {
            let mut load = Load::new(bus, p, q);
            load.in_service = mapper.in_service(id);
            load.uid = Some(id.to_string());
            loads.push(load);
        }
        count += 1;
    }
    if count > 0 {
        mapper.warnings.push_as(
            &codes::READ_CGMES_VALUE_APPROXIMATED,
            format!(
                "{count} EquivalentInjection(s) at boundary nodes mapped to \
             loads/generators (p/q at the tie point)"
            ),
        );
    }
}

fn read_branches(
    mapper: &mut Mapper<'_>,
    version: CgmesVersion,
) -> (Vec<Branch>, Vec<Transformer3W>) {
    let mut branches = Vec::new();
    for id in mapper.store.of_class("ACLineSegment") {
        let (Some(from), Some(to)) = (
            mapper.bus_of_equipment_terminal(id, 0),
            mapper.bus_of_equipment_terminal(id, 1),
        ) else {
            mapper.warnings.push(format!(
                "ACLineSegment {}: terminals do not land on two topological \
                 nodes (boundary line without its boundary set?); skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let store = mapper.store;
        let kv = mapper.kv(from);
        let z_base = kv * kv / SYSTEM_MVA;
        let y_base = SYSTEM_MVA / (kv * kv);
        let mut branch = Branch::new(
            from,
            to,
            store.f(id, "ACLineSegment.r").unwrap_or(0.0) / z_base,
            store.f(id, "ACLineSegment.x").unwrap_or(0.0) / z_base,
        );
        branch.b = store.f(id, "ACLineSegment.bch").unwrap_or(0.0) / y_base;
        let g = store.f(id, "ACLineSegment.gch").unwrap_or(0.0) / y_base;
        if g != 0.0 {
            let half_b = branch.b / 2.0;
            branch.charging = Some(BranchCharging::new(g / 2.0, half_b, g / 2.0, half_b));
        }
        branch.in_service = mapper.in_service(id);
        branch.uid = Some(id.to_string());
        apply_limits(mapper, id, &mut branch, kv, version);
        branches.push(branch);
    }
    for id in mapper.store.of_class("SeriesCompensator") {
        let (Some(from), Some(to)) = (
            mapper.bus_of_equipment_terminal(id, 0),
            mapper.bus_of_equipment_terminal(id, 1),
        ) else {
            mapper.warnings.push(format!(
                "SeriesCompensator {}: terminals do not land on two topological nodes; skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let kv = mapper.kv(from);
        let z_base = kv * kv / SYSTEM_MVA;
        let mut branch = Branch::new(
            from,
            to,
            mapper.store.f(id, "SeriesCompensator.r").unwrap_or(0.0) / z_base,
            mapper.store.f(id, "SeriesCompensator.x").unwrap_or(0.0) / z_base,
        );
        branch.in_service = mapper.in_service(id);
        branch.uid = Some(id.to_string());
        apply_limits(mapper, id, &mut branch, kv, version);
        branches.push(branch);
    }
    let transformers_3w = read_transformers(mapper, &mut branches, version);
    (branches, transformers_3w)
}

fn read_transformers(
    mapper: &mut Mapper<'_>,
    branches: &mut Vec<Branch>,
    version: CgmesVersion,
) -> Vec<Transformer3W> {
    // Ends grouped per transformer, ordered by endNumber.
    let mut ends_of: BTreeMap<String, Vec<(f64, String)>> = BTreeMap::new();
    for end in mapper.store.of_class("PowerTransformerEnd") {
        if let Some(xf) = mapper
            .store
            .refv(end, "PowerTransformerEnd.PowerTransformer")
        {
            let number = mapper
                .store
                .f(end, "TransformerEnd.endNumber")
                .unwrap_or(1.0);
            ends_of
                .entry(xf.to_string())
                .or_default()
                .push((number, end.to_string()));
        }
    }
    let mut transformers_3w = Vec::new();
    let mut unsupported = 0usize;
    for (xf, mut ends) in ends_of {
        ends.sort_by(|a, b| a.0.total_cmp(&b.0));
        if ends.len() == 3 {
            if let Some(transformer) = read_three_winding_transformer(mapper, &xf, &ends, version) {
                transformers_3w.push(transformer);
            }
            continue;
        }
        if ends.len() != 2 {
            unsupported += 1;
            continue;
        }
        let (end1, end2) = (ends[0].1.as_str(), ends[1].1.as_str());
        if let Some(branch) = read_two_winding_transformer(mapper, &xf, end1, end2, version) {
            branches.push(branch);
        }
    }
    if unsupported > 0 {
        mapper.warnings.push(format!(
            "{unsupported} PowerTransformer record(s) have neither two nor three windings and were skipped"
        ));
    }
    transformers_3w
}

fn read_two_winding_transformer(
    mapper: &mut Mapper<'_>,
    transformer_id: &str,
    end1: &str,
    end2: &str,
    version: CgmesVersion,
) -> Option<Branch> {
    let store = mapper.store;
    let end_terminal = |end: &str| {
        store
            .refv(end, "TransformerEnd.Terminal")
            .and_then(|terminal| mapper.wiring.node(terminal))
            .and_then(|node| mapper.bus_of_node.get(node))
            .copied()
    };
    let (Some(from), Some(to)) = (end_terminal(end1), end_terminal(end2)) else {
        mapper.warnings.push(format!(
            "PowerTransformer {}: ends do not land on topological nodes; skipped",
            store.name(transformer_id)
        ));
        return None;
    };

    let u1 = transformer_end_rated_kv(mapper, transformer_id, end1, from);
    let u2 = transformer_end_rated_kv(mapper, transformer_id, end2, to);
    let pu = |end: &str, key: &str, u: f64| {
        let u = if u > 0.0 { u } else { 1.0 };
        store.f(end, key).unwrap_or(0.0) / (u * u / SYSTEM_MVA)
    };
    let r = pu(end1, "PowerTransformerEnd.r", u1) + pu(end2, "PowerTransformerEnd.r", u2);
    let x = pu(end1, "PowerTransformerEnd.x", u1) + pu(end2, "PowerTransformerEnd.x", u2);
    let mut branch = Branch::new(from, to, r, x);
    let y_pu = |end: &str, property: &str, u: f64| {
        let u = if u > 0.0 { u } else { 1.0 };
        store.f(end, property).unwrap_or(0.0) * (u * u / SYSTEM_MVA)
    };
    let (g1, g2) = (
        y_pu(end1, "PowerTransformerEnd.g", u1),
        y_pu(end2, "PowerTransformerEnd.g", u2),
    );
    let (b1, b2) = (
        y_pu(end1, "PowerTransformerEnd.b", u1),
        y_pu(end2, "PowerTransformerEnd.b", u2),
    );
    if g1 != 0.0 || b1 != 0.0 || g2 != 0.0 || b2 != 0.0 {
        branch.b = b1 + b2;
        branch.charging = Some(BranchCharging::new(g1, b1, g2, b2));
    }

    let (kv1, kv2) = (mapper.kv(from), mapper.kv(to));
    let mut tap = safe_div(u1, kv1) / safe_div(u2, kv2);
    for (end, invert) in [(end1, false), (end2, true)] {
        if let Some(ratio_tap) = ratio_tap_changer(store, end) {
            let factor = ratio_tap_factor(mapper, ratio_tap);
            if invert {
                tap /= factor;
            } else {
                tap *= factor;
            }
        }
        if let Some(phase_tap) = phase_tap_changer(store, end) {
            branch.shift += phase_shift_deg(mapper, &phase_tap, invert);
        }
    }
    branch.tap = tap;
    let end_connected = |end: &str| {
        store
            .refv(end, "TransformerEnd.Terminal")
            .and_then(|terminal| mapper.wiring.connected.get(terminal))
            .copied()
            .unwrap_or(true)
    };
    let ends_connected = end_connected(end1) && end_connected(end2);
    branch.in_service = mapper.in_service(transformer_id) && ends_connected;
    branch.uid = Some(transformer_id.to_string());
    apply_limits(mapper, transformer_id, &mut branch, kv1, version);
    for end in [end1, end2] {
        apply_limits(mapper, end, &mut branch, kv1, version);
    }
    Some(branch)
}

fn read_three_winding_transformer(
    mapper: &mut Mapper<'_>,
    transformer_id: &str,
    ends: &[(f64, String)],
    version: CgmesVersion,
) -> Option<Transformer3W> {
    let end_ids = [ends[0].1.clone(), ends[1].1.clone(), ends[2].1.clone()];
    let (terminals, buses) = transformer_end_buses(mapper, transformer_id, &end_ids)?;

    let mut windings = [
        Winding::new(buses[0]),
        Winding::new(buses[1]),
        Winding::new(buses[2]),
    ];
    let mut star_r = [0.0; 3];
    let mut star_x = [0.0; 3];
    let mut mag_g = 0.0;
    let mut mag_b = 0.0;
    for index in 0..3 {
        let end = &end_ids[index];
        let bus = buses[index];
        let rated_kv = transformer_end_rated_kv(mapper, transformer_id, end, bus);
        let z_base = rated_kv * rated_kv / SYSTEM_MVA;
        star_r[index] = mapper.store.f(end, "PowerTransformerEnd.r").unwrap_or(0.0) / z_base;
        star_x[index] = mapper.store.f(end, "PowerTransformerEnd.x").unwrap_or(0.0) / z_base;
        mag_g += mapper.store.f(end, "PowerTransformerEnd.g").unwrap_or(0.0) * z_base;
        mag_b += mapper.store.f(end, "PowerTransformerEnd.b").unwrap_or(0.0) * z_base;

        let mut tap = safe_div(rated_kv, mapper.kv(bus));
        if let Some(ratio_tap) = ratio_tap_changer(mapper.store, end) {
            tap *= ratio_tap_factor(mapper, ratio_tap);
        }
        let shift = phase_tap_changer(mapper.store, end)
            .map_or(0.0, |phase_tap| phase_shift_deg(mapper, &phase_tap, false));

        let mut limits = Branch::new(bus, bus, 0.0, 1.0);
        apply_limits_to_targets(
            mapper.store,
            &[terminals[index].as_str(), end.as_str()],
            &mut limits,
            mapper.kv(bus),
            version,
        );
        windings[index] = Winding {
            bus,
            tap,
            shift,
            nominal_kv: rated_kv,
            rate_a: limits.rate_a,
            rate_b: limits.rate_b,
            rate_c: limits.rate_c,
            control: None,
        };
    }

    let impedance = |a: usize, b: usize| {
        Impedance::new(star_r[a] + star_r[b], star_x[a] + star_x[b], SYSTEM_MVA)
    };
    let connected = terminals.iter().all(|terminal| {
        mapper
            .wiring
            .connected
            .get(terminal)
            .copied()
            .unwrap_or(true)
    });
    let mut transformer = Transformer3W::new(
        windings,
        [impedance(0, 1), impedance(1, 2), impedance(2, 0)],
    );
    transformer.mag_g = mag_g;
    transformer.mag_b = mag_b;
    transformer.in_service = mapper.in_service(transformer_id) && connected;
    transformer.name = Some(mapper.store.name(transformer_id));
    transformer.uid = Some(transformer_id.to_string());
    Some(transformer)
}

fn transformer_end_rated_kv(
    mapper: &mut Mapper<'_>,
    transformer_id: &str,
    end: &str,
    bus: BusId,
) -> f64 {
    if let Some(value) = mapper.store.f(end, "PowerTransformerEnd.ratedU")
        && value.is_finite()
        && value > 0.0
    {
        return value;
    }
    let fallback = mapper.kv(bus);
    let source = mapper
        .store
        .text(end, "PowerTransformerEnd.ratedU")
        .map_or("is absent".to_string(), |value| format!("is `{value}`"));
    mapper.warnings.push_as(&codes::READ_CGMES_VALUE_DEFAULTED, format!(
        "PowerTransformer `{transformer_id}` end `{end}` PowerTransformerEnd.ratedU {source}; used the connected topological node base voltage {fallback} kV for impedance conversion"
    ));
    fallback
}

fn transformer_end_buses(
    mapper: &mut Mapper<'_>,
    transformer_id: &str,
    end_ids: &[String; 3],
) -> Option<(Vec<String>, Vec<BusId>)> {
    let mut terminals = Vec::with_capacity(3);
    let mut buses = Vec::with_capacity(3);
    for end in end_ids {
        let Some(terminal) = mapper
            .store
            .refv(end, "TransformerEnd.Terminal")
            .map(str::to_string)
        else {
            mapper.warnings.push(format!(
                "PowerTransformer {}: winding {} has no terminal; skipped",
                mapper.store.name(transformer_id),
                terminals.len() + 1
            ));
            return None;
        };
        let Some(bus) = mapper
            .wiring
            .node(&terminal)
            .and_then(|node| mapper.bus_of_node.get(node))
            .copied()
        else {
            mapper.warnings.push(format!(
                "PowerTransformer {}: winding {} does not land on a topological node; skipped",
                mapper.store.name(transformer_id),
                terminals.len() + 1
            ));
            return None;
        };
        terminals.push(terminal);
        buses.push(bus);
    }
    Some((terminals, buses))
}

fn safe_div(a: f64, b: f64) -> f64 {
    if a > 0.0 && b > 0.0 { a / b } else { 1.0 }
}

/// The in-effect ratio factor of a ratio tap changer: SSH step, else the SV
/// tap step, else neutral.
fn ratio_tap_factor(mapper: &Mapper<'_>, rtc: &str) -> f64 {
    let store = mapper.store;
    let neutral = store.f(rtc, "TapChanger.neutralStep").unwrap_or(0.0);
    let step = store
        .f(rtc, "TapChanger.step")
        .or_else(|| sv_tap_step(store, rtc))
        .unwrap_or(neutral);
    if let Some(steps) = table_tap_steps(store, rtc, TapChangerKind::Ratio)
        && let Some(value) = steps
            .iter()
            .find(|value| value.position == step.round() as i32)
    {
        return value.rho;
    }
    let increment = store
        .f(rtc, "RatioTapChanger.stepVoltageIncrement")
        .unwrap_or(0.0);
    1.0 + (step - neutral) * increment / 100.0
}

fn sv_tap_step(store: &Store, tap_changer: &str) -> Option<f64> {
    store.of_class("SvTapStep").find_map(|sv| {
        (store.refv(sv, "SvTapStep.TapChanger") == Some(tap_changer))
            .then(|| store.f(sv, "SvTapStep.position"))
            .flatten()
    })
}

fn ratio_tap_changer<'a>(store: &'a Store, end: &str) -> Option<&'a str> {
    store
        .refv(end, "TransformerEnd.RatioTapChanger")
        .or_else(|| {
            store
                .of_class("RatioTapChanger")
                .find(|tap| store.refv(tap, "RatioTapChanger.TransformerEnd") == Some(end))
        })
}

fn phase_tap_changer(store: &Store, end: &str) -> Option<String> {
    store
        .refv(end, "TransformerEnd.PhaseTapChanger")
        .or_else(|| {
            [
                "PhaseTapChangerLinear",
                "PhaseTapChangerSymmetrical",
                "PhaseTapChangerAsymmetrical",
                "PhaseTapChangerTabular",
            ]
            .into_iter()
            .find_map(|class| {
                store
                    .of_class(class)
                    .find(|tap| store.refv(tap, "PhaseTapChanger.TransformerEnd") == Some(end))
            })
        })
        .map(str::to_string)
}

/// Best-effort phase shift in degrees. The linear changer is exact; the
/// asymmetrical/symmetrical/tabular families warn and use what they can.
fn phase_shift_deg(mapper: &mut Mapper<'_>, ptc: &str, invert: bool) -> f64 {
    let store = mapper.store;
    let class = store.class_of(ptc).unwrap_or("PhaseTapChanger").to_string();
    let neutral = store.f(ptc, "TapChanger.neutralStep").unwrap_or(0.0);
    let step = store
        .f(ptc, "TapChanger.step")
        .or_else(|| sv_tap_step(store, ptc))
        .unwrap_or(neutral);
    let degrees = match class.as_str() {
        "PhaseTapChangerLinear" => {
            (step - neutral)
                * store
                    .f(ptc, "PhaseTapChangerLinear.stepPhaseShiftIncrement")
                    .unwrap_or(0.0)
        }
        "PhaseTapChangerTabular" => table_tap_steps(store, ptc, TapChangerKind::Phase)
            .and_then(|steps| {
                steps
                    .into_iter()
                    .find(|value| value.position == step.round() as i32)
            })
            .map_or(0.0, |value| value.alpha_degrees),
        "PhaseTapChangerSymmetrical" | "PhaseTapChangerAsymmetrical" => {
            calculated_tap_steps(store, ptc, TapChangerKind::Phase)
                .ok()
                .and_then(|steps| {
                    steps
                        .into_iter()
                        .find(|value| value.position == step.round() as i32)
                })
                .map_or(0.0, |value| value.alpha_degrees)
        }
        other => {
            mapper.warnings.push(format!(
                "{other} {}: phase tap changer type is not supported in the calculation projection; using zero shift",
                store.name(ptc)
            ));
            0.0
        }
    };
    if invert { -degrees } else { degrees }
}

/// PATL → `rate_a`; a TATL admissible for longer than a minute → `rate_b` and
/// one admissible for a minute or less → `rate_c`, the short term and
/// emergency ratings the reference importer reads the same two durations as; a
/// tripping current kind also lands on `rate_c`. Limits come from the
/// operational limit sets on the equipment's terminals (or the equipment
/// itself). Current limits convert through √3·kV; apparent/active limits are
/// MVA/MW.
fn apply_limits(
    mapper: &mut Mapper<'_>,
    equipment: &str,
    branch: &mut Branch,
    kv: f64,
    version: CgmesVersion,
) {
    let mut terminals: Vec<&str> = mapper
        .wiring
        .terminals(equipment)
        .iter()
        .map(String::as_str)
        .collect();
    terminals.push(equipment);
    apply_limits_to_targets(mapper.store, &terminals, branch, kv, version);
}

/// A temporary limit admissible for this many seconds or fewer is the
/// emergency rating, as the reference importer classifies it.
const EMERGENCY_LIMIT_SECONDS: f64 = 60.0;

fn apply_limits_to_targets(
    store: &Store,
    targets: &[&str],
    branch: &mut Branch,
    kv: f64,
    version: CgmesVersion,
) {
    // Limit sets point at their terminal/equipment, so scan sets once.
    for set in store.of_class("OperationalLimitSet") {
        let target = store
            .refv(set, "OperationalLimitSet.Terminal")
            .or_else(|| store.refv(set, "OperationalLimitSet.Equipment"));
        if !target.is_some_and(|target| targets.contains(&target)) {
            continue;
        }
        for limit in store.objects.iter().filter_map(|o| {
            matches!(
                o.class.as_str(),
                "CurrentLimit" | "ApparentPowerLimit" | "ActivePowerLimit"
            )
            .then_some(o.id.as_str())
        }) {
            if store.refv(limit, "OperationalLimit.OperationalLimitSet") != Some(set) {
                continue;
            }
            let Some(limit_type) = store.refv(limit, "OperationalLimit.OperationalLimitType")
            else {
                continue;
            };
            let kind = match version {
                CgmesVersion::V2_4_15 => {
                    store.enum_value(limit_type, "entsoe:OperationalLimitType.limitType")
                }
                CgmesVersion::V3_0 => store.enum_value(limit_type, "eu:OperationalLimitType.kind"),
            };
            let class = store.class_of(limit).unwrap_or_default();
            let value = store
                .f(limit, &format!("{class}.value"))
                .or_else(|| store.f(limit, &format!("{class}.normalValue")))
                .unwrap_or(0.0);
            let mva = if class == "CurrentLimit" {
                3f64.sqrt() * kv * value / 1000.0
            } else {
                value
            };
            let slot = match kind {
                Some("patl") => Some(&mut branch.rate_a),
                Some("tatl") => {
                    if store
                        .f(limit_type, "OperationalLimitType.acceptableDuration")
                        .is_some_and(|seconds| seconds <= EMERGENCY_LIMIT_SECONDS)
                    {
                        Some(&mut branch.rate_c)
                    } else {
                        Some(&mut branch.rate_b)
                    }
                }
                Some("tc" | "tct") => Some(&mut branch.rate_c),
                _ => None,
            };
            if let Some(slot) = slot {
                // Several sets can constrain one branch; the binding limit
                // is the smallest.
                if *slot == 0.0 || mva < *slot {
                    *slot = mva;
                }
            }
        }
    }
}

fn class_is_consumed(class: &str) -> bool {
    CONSUMED.contains(&class)
        || SWITCH_CLASSES.contains(&class)
        || LOAD_CLASSES.contains(&class)
        || matches!(
            class,
            "CurrentLimit" | "ActivePowerLimit" | "ApparentPowerLimit" | "VoltageLimit"
        )
        || matches!(
            class,
            // Containment and administrative records used while building
            // hierarchy, controls, shunts, and operational limits.
            "Substation"
                | "VoltageLevel"
                | "BusbarSection"
                | "RegulatingControl"
                | "TapChangerControl"
                | "LoadResponseCharacteristic"
                | "OperationalLimitSet"
                | "OperationalLimitType"
                | "NonlinearShuntCompensator"
                | "NonlinearShuntCompensatorPoint"
        )
}

fn normalized_identity_value(raw: &str) -> &str {
    let value = raw.trim();
    let value = value.strip_prefix("urn:uuid:").unwrap_or(value);
    let value = value
        .rsplit_once('#')
        .map_or(value, |(_, fragment)| fragment);
    value.strip_prefix('_').unwrap_or(value)
}

/// Warn for every unconsumed class and every unconsumed field on a consumed
/// class. Property access is tracked by exact object and property position so
/// repeated RDF properties cannot hide an unread value.
/// Whether every declared modeling authority is this writer's own.
fn is_own_output(store: &Store) -> bool {
    store.own_output && !store.foreign_output
}

/// Unmapped container classes fresh emission creates to make a complete
/// equipment hierarchy. No source-neutral record supplies their identity or
/// fields.
fn synthesized_unmapped_class(class: &str) -> bool {
    matches!(class, "GeographicalRegion" | "SubGeographicalRegion")
}

/// Properties fresh emission derives solely to connect its generated CGMES
/// hierarchy and state records.
fn synthesized_unmapped_property(class: &str, property: &str) -> bool {
    matches!(
        (class, property),
        ("GeneratingUnit", "Equipment.inService")
            | ("OperationalLimitType", "OperationalLimitType.direction")
            | (
                "LinearShuntCompensator",
                "ShuntCompensator.nomU" | "ShuntCompensator.normalSections"
            )
            | ("Substation", "Substation.Region")
            | ("TopologicalIsland", "TopologicalIsland.TopologicalNodes")
    )
}

/// Classes whose mRID identifies a record subordinate to source-neutral
/// equipment. Fresh emission derives these identities from their owner.
fn synthesized_subordinate_identity(class: &str) -> bool {
    matches!(
        class,
        "TapChangerControl"
            | "RatioTapChangerTable"
            | "RatioTapChangerTablePoint"
            | "PhaseTapChangerTable"
            | "PhaseTapChangerTablePoint"
            | "ReactiveCapabilityCurve"
            | "VsCapabilityCurve"
            | "CurveData"
            | "OperationalLimitType"
            | "CurrentLimit"
            | "ActivePowerLimit"
            | "ApparentPowerLimit"
    )
}

/// Metadata for records whose semantic content is retained by operational
/// limit groups while fresh emission derives their RDF identity.
fn writer_derived_component_metadata(class: &str) -> bool {
    matches!(
        class,
        "OperationalLimitSet"
            | "OperationalLimitType"
            | "CurrentLimit"
            | "ActivePowerLimit"
            | "ApparentPowerLimit"
    )
}

fn warn_unmapped(store: &Store, warnings: &mut CgmesDiagnostics) {
    let own_output = is_own_output(store);
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut fields: BTreeMap<(&str, &str), (usize, Vec<&str>)> = BTreeMap::new();
    let read_props = store.read_props.borrow();
    for (object_at, object) in store.objects.iter().enumerate() {
        let class = object.class.as_str();
        if !class_is_consumed(class) {
            if own_output && synthesized_unmapped_class(class) {
                continue;
            }
            *counts.entry(class).or_default() += 1;
            continue;
        }
        for (property_at, (property, value)) in object.props.iter().enumerate() {
            if read_props.contains(&(object_at, property_at)) {
                continue;
            }
            if own_output && synthesized_unmapped_property(class, property) {
                continue;
            }
            if property == "IdentifiedObject.mRID" {
                if matches!(value, PropValue::Text(value) if normalized_identity_value(value) == object.id)
                {
                    continue;
                }
                warnings.push_as(
                    &codes::READ_CGMES_FIELD_UNMAPPED,
                    format!(
                        "{} `{}` property `IdentifiedObject.mRID` is {}, but its RDF identity is `{}`; a CIM object has one identity in the balanced calculation view, taken from the RDF identity, and fresh CGMES output replaces this mRID",
                        object.class,
                        object.id,
                        describe_property_value(value),
                        object.id,
                    ),
                );
                continue;
            }
            let (occurrences, ids) = fields
                .entry((class, property.as_str()))
                .or_insert_with(|| (0, Vec::new()));
            *occurrences += 1;
            if !ids.contains(&object.id.as_str()) {
                ids.push(object.id.as_str());
            }
        }
    }
    for (class, count) in counts {
        warnings.push(format!(
            "{count} {class} object(s) state no electrical value and no container the balanced calculation view holds; their identity metadata is retained and nothing else"
        ));
    }
    for ((class, property), (occurrences, ids)) in fields {
        let samples = ids.iter().take(5).copied().collect::<Vec<_>>().join("`, `");
        let remainder = if ids.len() > 5 {
            format!(" and {} more", ids.len() - 5)
        } else {
            String::new()
        };
        warnings.push_as(
            &codes::READ_CGMES_FIELD_UNMAPPED,
            format!(
                "{} {class} object(s) state {occurrences} `{property}` value(s) that no field of the balanced calculation view holds (objects: [`{samples}`]{remainder}); fresh CGMES output omits this field",
                ids.len(),
            ),
        );
    }
}

fn warn_regenerated_subordinate_identities(store: &Store, warnings: &mut CgmesDiagnostics) {
    for class in [
        "TapChangerControl",
        "RatioTapChangerTable",
        "RatioTapChangerTablePoint",
        "PhaseTapChangerTable",
        "PhaseTapChangerTablePoint",
        "ReactiveCapabilityCurve",
        "VsCapabilityCurve",
        "CurveData",
        "OperationalLimitType",
        "CurrentLimit",
        "ActivePowerLimit",
        "ApparentPowerLimit",
    ] {
        if is_own_output(store) && synthesized_subordinate_identity(class) {
            continue;
        }
        let ids = store.of_class(class).collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let sample = ids.iter().take(5).copied().collect::<Vec<_>>().join("`, `");
        let remainder = if ids.len() > 5 {
            format!(" and {} more", ids.len() - 5)
        } else {
            String::new()
        };
        let (identity, verb) = if ids.len() == 1 {
            ("identity", "is")
        } else {
            ("identities", "are")
        };
        warnings.push(format!(
            "{} {class} {identity} [`{sample}`]{remainder} {verb} the mRID of an object subordinate to an element, and the balanced calculation view identifies elements and not their subordinate objects; the electrical values and relationships are retained, and fresh CGMES assigns deterministic subordinate mRIDs",
            ids.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document_with_literal(class: &str, property: &str, value: &str) -> CimDocument {
        CimDocument {
            cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
            header: None,
            objects: vec![CimObject {
                class: class.into(),
                id: "bad-value".into(),
                definition: true,
                props: vec![(property.into(), PropValue::Text(value.into()))],
            }],
        }
    }

    fn document_with_property(definition: bool, value: PropValue) -> CimDocument {
        CimDocument {
            cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
            header: None,
            objects: vec![CimObject {
                class: "EnergyConsumer".into(),
                id: "load".into(),
                definition,
                props: vec![("EnergyConsumer.p".into(), value)],
            }],
        }
    }

    #[test]
    fn coalesces_equal_properties_and_rejects_conflicting_profile_values() {
        let mut store = Store::default();
        store
            .merge(document_with_property(true, PropValue::Text("10".into())))
            .unwrap();
        store
            .merge(document_with_property(false, PropValue::Text("10".into())))
            .unwrap();
        assert_eq!(store.objects[0].props.len(), 1);

        let error = store
            .merge(document_with_property(false, PropValue::Text("11".into())))
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("RDF object `load`"));
        assert!(message.contains("EnergyConsumer.p"));
        assert!(message.contains("text `10`"));
        assert!(message.contains("text `11`"));

        let mut island = Vec::new();
        merge_properties(
            "island",
            &mut island,
            vec![
                (
                    "TopologicalIsland.TopologicalNodes".into(),
                    PropValue::Ref("node-1".into()),
                ),
                (
                    "TopologicalIsland.TopologicalNodes".into(),
                    PropValue::Ref("node-2".into()),
                ),
            ],
        )
        .unwrap();
        assert_eq!(island.len(), 2);
    }

    #[test]
    fn rejects_malformed_and_nonfinite_electrical_literals() {
        for (class, property, value) in [
            ("EnergyConsumer", "EnergyConsumer.p", "not-a-number"),
            ("EnergyConsumer", "EnergyConsumer.q", "NaN"),
            ("BaseVoltage", "BaseVoltage.nominalVoltage", "+inf"),
            ("SvVoltage", "SvVoltage.v", "NaN"),
            ("SvVoltage", "SvVoltage.angle", "bad-angle"),
            ("ACLineSegment", "ACLineSegment.r", "NaN"),
            ("PowerTransformerEnd", "PowerTransformerEnd.x", "bad-x"),
            ("RotatingMachine", "RotatingMachine.ratedS", "NaN"),
            ("RotatingMachine", "RotatingMachine.ratedS", "bad-rating"),
            ("Switch", "Switch.ratedCurrent", "inf"),
            ("CurrentLimit", "CurrentLimit.value", "NaN"),
            ("TapChanger", "TapChanger.step", "-inf"),
        ] {
            let mut store = Store::default();
            let error = store
                .merge(document_with_literal(class, property, value))
                .unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains(property),
                "error did not identify {property}: {message}"
            );
            assert!(
                message.contains(value),
                "error did not identify `{value}`: {message}"
            );
            assert!(
                message.contains("finite number") || message.contains("expected an integer"),
                "error did not describe the numeric requirement: {message}"
            );
        }
    }

    #[test]
    fn rejects_malformed_boolean_literals() {
        for (class, property, value) in [
            ("Switch", "Switch.open", "closed"),
            (
                "OperationalLimitType",
                "OperationalLimitType.isInfiniteDuration",
                "yes",
            ),
            ("TapChanger", "TapChanger.ltcFlag", "on"),
        ] {
            let mut store = Store::default();
            let error = store
                .merge(document_with_literal(class, property, value))
                .unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains(property),
                "error did not identify {property}: {message}"
            );
            assert!(
                message.contains(value),
                "error did not identify `{value}`: {message}"
            );
            assert!(
                message.contains("true, false, 1, or 0"),
                "error did not describe the boolean requirement: {message}"
            );
        }
    }

    #[test]
    fn rejects_fractional_and_out_of_range_integer_literals() {
        for (class, property, value) in [
            ("Terminal", "ACDCTerminal.sequenceNumber", "1.5"),
            ("Terminal", "ACDCTerminal.sequenceNumber", "256"),
            ("ACDCConverter", "ACDCConverter.numberOfValves", "-1"),
            ("ACDCConverter", "ACDCConverter.numberOfValves", "1.5"),
            (
                "ACDCConverter",
                "ACDCConverter.numberOfValves",
                "4294967296",
            ),
            ("TapChanger", "TapChanger.lowStep", "2147483648"),
            (
                "RatioTapChangerTablePoint",
                "TapChangerTablePoint.step",
                "-2147483649",
            ),
            ("PowerTransformerEnd", "TransformerEnd.endNumber", "0"),
            ("PowerTransformerEnd", "TransformerEnd.endNumber", "256"),
            (
                "NonlinearShuntCompensatorPoint",
                "NonlinearShuntCompensatorPoint.sectionNumber",
                "0.5",
            ),
            (
                "NonlinearShuntCompensatorPoint",
                "NonlinearShuntCompensatorPoint.sectionNumber",
                "0",
            ),
            (
                "LinearShuntCompensator",
                "ShuntCompensator.maximumSections",
                "-1",
            ),
        ] {
            let mut store = Store::default();
            let message = store
                .merge(document_with_literal(class, property, value))
                .unwrap_err()
                .to_string();
            assert!(
                message.contains(property),
                "error did not identify {property}: {message}"
            );
            assert!(
                message.contains(value),
                "error did not identify `{value}`: {message}"
            );
            assert!(
                message.contains("integer"),
                "error did not describe the integer requirement: {message}"
            );
        }
    }

    #[test]
    fn accepts_cgmes_continuous_section_tap_and_duration_values() {
        for (class, property) in [
            ("LinearShuntCompensator", "ShuntCompensator.sections"),
            (
                "SvShuntCompensatorSections",
                "SvShuntCompensatorSections.sections",
            ),
            ("RatioTapChanger", "TapChanger.step"),
            ("SvTapStep", "SvTapStep.position"),
            (
                "OperationalLimitType",
                "OperationalLimitType.acceptableDuration",
            ),
        ] {
            let mut store = Store::default();
            store
                .merge(document_with_literal(class, property, "2.5"))
                .unwrap();
        }
    }

    #[test]
    fn diagnoses_full_model_fields_that_do_not_enter_the_electrical_model() {
        let header = ModelHeader {
            identity: Some("model-eq".into()),
            profiles: vec!["http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0".into()],
            modeling_authority_set: Some("https://example.test/operator".into()),
            description: Some("case name".into()),
            scenario_time: Some("2026-01-02T00:00:00Z".into()),
            created: Some("2026-01-03T00:00:00Z".into()),
            version: Some("9".into()),
            dependent_on: vec!["model-boundary".into()],
            unmapped_properties: vec![
                ("md:Model.Supersedes".into(), PropValue::Ref("older".into())),
                (
                    "md:Model.Supersedes".into(),
                    PropValue::Ref("oldest".into()),
                ),
            ],
            nested_properties: vec!["md:Model.custom".into()],
        };
        let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
        warn_full_model_fields("grid_EQ.xml", Some(&header), &mut warnings);

        for expected in [
            "FullModel RDF identity `model-eq`",
            "`Model.created`",
            "`Model.version`",
            "1 `Model.DependentOn` reference",
            "`md:Model.Supersedes` (2)",
            "nested RDF/XML: `md:Model.custom` (1)",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(expected)),
                "{expected}"
            );
        }
        assert!(
            warnings
                .iter()
                .all(|warning| warning.info.code == codes::READ_CGMES_FIELD_UNMAPPED.code)
        );
    }

    #[test]
    fn skipped_presentation_parts_report_classes_counts_and_sample_ids() {
        let document = CimDocument {
            cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
            header: Some(ModelHeader {
                profiles: vec!["http://iec.ch/TC57/ns/CIM/DiagramLayout-EU/3.0".into()],
                ..ModelHeader::default()
            }),
            objects: vec![
                CimObject {
                    class: "Diagram".into(),
                    id: "diagram-1".into(),
                    definition: true,
                    props: Vec::new(),
                },
                CimObject {
                    class: "DiagramObject".into(),
                    id: "object-1".into(),
                    definition: true,
                    props: Vec::new(),
                },
                CimObject {
                    class: "DiagramObject".into(),
                    id: "object-2".into(),
                    definition: true,
                    props: Vec::new(),
                },
            ],
        };
        let summary = skipped_part_summary("grid_DL.xml", &document);
        assert!(summary.contains("grid_DL.xml"));
        assert!(summary.contains("DiagramLayout"));
        assert!(summary.contains("Diagram: 1 [`diagram-1`]"));
        assert!(summary.contains("DiagramObject: 2 [`object-1`, `object-2`]"));
    }

    #[test]
    fn noncanonical_limit_kinds_and_fractional_durations_are_explicit() {
        let mut store = Store::default();
        store
            .merge(CimDocument {
                cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
                header: None,
                objects: vec![
                    CimObject {
                        class: "OperationalLimitSet".into(),
                        id: "set".into(),
                        definition: true,
                        props: Vec::new(),
                    },
                    CimObject {
                        class: "OperationalLimitType".into(),
                        id: "type".into(),
                        definition: true,
                        props: vec![
                            (
                                "eu:OperationalLimitType.kind".into(),
                                PropValue::Ref("LimitKind.patlt".into()),
                            ),
                            (
                                "OperationalLimitType.isInfiniteDuration".into(),
                                PropValue::Text("false".into()),
                            ),
                            (
                                "OperationalLimitType.acceptableDuration".into(),
                                PropValue::Text("12.5".into()),
                            ),
                        ],
                    },
                    CimObject {
                        class: "CurrentLimit".into(),
                        id: "limit".into(),
                        definition: true,
                        props: vec![
                            (
                                "OperationalLimit.OperationalLimitSet".into(),
                                PropValue::Ref("set".into()),
                            ),
                            (
                                "OperationalLimit.OperationalLimitType".into(),
                                PropValue::Ref("type".into()),
                            ),
                            (
                                "CurrentLimit.normalValue".into(),
                                PropValue::Text("1000".into()),
                            ),
                        ],
                    },
                ],
            })
            .unwrap();
        let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
        let limits = loading_limits(
            &store,
            "set",
            "CurrentLimit",
            CgmesVersion::V3_0,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(limits.temporary_limits[0].acceptable_duration_seconds, 13);
        assert!(warnings.iter().any(|warning| {
            warning.contains("kind `patlt`") && warning.contains("fresh CGMES emits kind `tatl`")
        }));
        assert!(warnings.iter().any(|warning| {
            warning.contains("acceptableDuration=12.5") && warning.contains("13 whole seconds")
        }));
    }

    #[test]
    fn invalid_temporary_limit_durations_are_explicit() {
        for duration_value in ["-1", "18446744073709551616"] {
            let mut store = Store::default();
            store
                .merge(CimDocument {
                    cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
                    header: None,
                    objects: vec![
                        CimObject {
                            class: "OperationalLimitSet".into(),
                            id: "set".into(),
                            definition: true,
                            props: Vec::new(),
                        },
                        CimObject {
                            class: "OperationalLimitType".into(),
                            id: "type".into(),
                            definition: true,
                            props: vec![
                                (
                                    "OperationalLimitType.isInfiniteDuration".into(),
                                    PropValue::Text("false".into()),
                                ),
                                (
                                    "OperationalLimitType.acceptableDuration".into(),
                                    PropValue::Text(duration_value.into()),
                                ),
                            ],
                        },
                        CimObject {
                            class: "CurrentLimit".into(),
                            id: "limit".into(),
                            definition: true,
                            props: vec![
                                (
                                    "OperationalLimit.OperationalLimitSet".into(),
                                    PropValue::Ref("set".into()),
                                ),
                                (
                                    "OperationalLimit.OperationalLimitType".into(),
                                    PropValue::Ref("type".into()),
                                ),
                                (
                                    "CurrentLimit.normalValue".into(),
                                    PropValue::Text("1000".into()),
                                ),
                            ],
                        },
                    ],
                })
                .unwrap();
            let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
            let limits = loading_limits(
                &store,
                "set",
                "CurrentLimit",
                CgmesVersion::V3_0,
                &mut warnings,
            )
            .unwrap();
            assert_eq!(limits.temporary_limits[0].acceptable_duration_seconds, 0);
            assert!(warnings.iter().any(|warning| {
                warning.contains("acceptableDuration=")
                    && warning.contains("cannot be represented")
                    && warning.contains("retains the limit with 0 seconds")
            }));
        }
    }

    #[test]
    fn distinct_equal_base_voltage_identities_are_diagnosed() {
        let mut store = Store::default();
        store
            .merge(CimDocument {
                cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
                header: None,
                objects: ["base-a", "base-b"]
                    .into_iter()
                    .map(|id| CimObject {
                        class: "BaseVoltage".into(),
                        id: id.into(),
                        definition: true,
                        props: vec![(
                            "BaseVoltage.nominalVoltage".into(),
                            PropValue::Text("230".into()),
                        )],
                    })
                    .collect(),
            })
            .unwrap();
        let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
        warn_collapsed_base_voltage_identities(&store, &mut warnings);
        assert!(warnings.iter().any(|warning| {
            warning.contains("2 distinct BaseVoltage identities")
                && warning.contains("`base-a`, `base-b`")
                && warning.contains("one deterministic BaseVoltage identity")
        }));
    }

    #[test]
    fn unsupported_tap_control_mode_is_not_silently_defaulted() {
        let mut store = Store::default();
        store
            .merge(CimDocument {
                cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
                header: None,
                objects: vec![
                    CimObject {
                        class: "RatioTapChanger".into(),
                        id: "tap".into(),
                        definition: true,
                        props: vec![(
                            "TapChanger.TapChangerControl".into(),
                            PropValue::Ref("control".into()),
                        )],
                    },
                    CimObject {
                        class: "TapChangerControl".into(),
                        id: "control".into(),
                        definition: true,
                        props: vec![(
                            "RegulatingControl.mode".into(),
                            PropValue::Ref("RegulatingControlModeKind.frequency".into()),
                        )],
                    },
                ],
            })
            .unwrap();
        let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
        assert_eq!(tap_control_mode(&store, "tap", &mut warnings), None);
        assert!(warnings.iter().any(|warning| {
            warning.info.code == codes::READ_CGMES_VALUE_APPROXIMATED.code
                && warning.contains("RegulatingControl.mode `frequency`")
                && warning.contains("fresh CGMES output selects the default")
        }));
    }

    #[test]
    fn rejects_resource_references_for_typed_literals() {
        let mut store = Store::default();
        let mut document = document_with_literal("EnergyConsumer", "EnergyConsumer.p", "1");
        document.objects[0].props[0].1 = PropValue::Ref("not-a-number".into());

        let message = store.merge(document).unwrap_err().to_string();
        assert!(message.contains("EnergyConsumer.p"));
        assert!(message.contains("RDF resource reference, not a typed literal"));
    }

    #[test]
    fn unknown_limit_tap_changer_and_generating_unit_classes_are_diagnosed() {
        let mut store = Store::default();
        store
            .merge(CimDocument {
                cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
                header: None,
                objects: [
                    "FutureVoltageLimit",
                    "VendorPhaseTapChanger",
                    "VendorGeneratingUnit",
                ]
                .into_iter()
                .map(|class| CimObject {
                    class: class.into(),
                    id: class.to_ascii_lowercase(),
                    definition: true,
                    props: Vec::new(),
                })
                .collect(),
            })
            .unwrap();
        let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
        warn_unmapped(&store, &mut warnings);

        for class in [
            "FutureVoltageLimit",
            "VendorPhaseTapChanger",
            "VendorGeneratingUnit",
        ] {
            assert!(warnings.iter().any(|warning| {
                warning.info.code == codes::READ_CGMES_RECORD_UNMAPPED.code
                    && warning.contains(&format!("1 {class} object(s)"))
                    && warning.contains("state no electrical value and no container")
            }));
        }
    }

    #[test]
    fn mapped_classes_report_each_unread_field_without_false_mrid_matches() {
        let mut store = Store::default();
        store
            .merge(CimDocument {
                cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
                header: None,
                objects: vec![
                    CimObject {
                        class: "EquivalentInjection".into(),
                        id: "equivalent".into(),
                        definition: true,
                        props: vec![
                            (
                                "IdentifiedObject.mRID".into(),
                                PropValue::Text("urn:uuid:equivalent".into()),
                            ),
                            ("EquivalentInjection.p".into(), PropValue::Text("10".into())),
                            (
                                "EquivalentInjection.r".into(),
                                PropValue::Text("0.1".into()),
                            ),
                            (
                                "IdentifiedObject.name".into(),
                                PropValue::Ref("not-a-literal".into()),
                            ),
                        ],
                    },
                    CimObject {
                        class: "TopologicalIsland".into(),
                        id: "island".into(),
                        definition: true,
                        props: vec![
                            (
                                "TopologicalIsland.TopologicalNodes".into(),
                                PropValue::Ref("node-1".into()),
                            ),
                            (
                                "TopologicalIsland.TopologicalNodes".into(),
                                PropValue::Ref("node-2".into()),
                            ),
                        ],
                    },
                    CimObject {
                        class: "SynchronousMachine".into(),
                        id: "machine".into(),
                        definition: true,
                        props: vec![(
                            "IdentifiedObject.mRID".into(),
                            PropValue::Text("different-machine-id".into()),
                        )],
                    },
                ],
            })
            .unwrap();

        assert_eq!(store.f("equivalent", "EquivalentInjection.p"), Some(10.0));
        assert_eq!(store.text("equivalent", "IdentifiedObject.name"), None);
        assert_eq!(
            store.refv("island", "TopologicalIsland.TopologicalNodes"),
            Some("node-1")
        );

        let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
        warn_unmapped(&store, &mut warnings);
        let fields = warnings
            .iter()
            .filter(|warning| warning.info.code == codes::READ_CGMES_FIELD_UNMAPPED.code)
            .collect::<Vec<_>>();

        assert_eq!(fields.len(), 4);
        assert!(fields.iter().any(|warning| {
            warning.contains("EquivalentInjection.r") && warning.contains("`equivalent`")
        }));
        assert!(fields.iter().any(|warning| {
            warning.contains("IdentifiedObject.name") && warning.contains("`equivalent`")
        }));
        assert!(fields.iter().any(|warning| {
            warning.contains("TopologicalIsland.TopologicalNodes")
                && warning.contains("1 `TopologicalIsland.TopologicalNodes` value")
        }));
        assert!(fields.iter().any(|warning| {
            warning.contains("SynchronousMachine `machine`")
                && warning.contains("different-machine-id")
                && warning.contains("RDF identity is `machine`")
        }));
        assert!(!fields.iter().any(|warning| {
            warning.contains("EquivalentInjection.p")
                || (warning.contains("IdentifiedObject.mRID")
                    && warning.contains("EquivalentInjection"))
        }));
    }

    #[test]
    fn powerio_authority_suppresses_only_values_fresh_emission_synthesizes() {
        let mut store = Store {
            own_output: true,
            ..Store::default()
        };
        store
            .merge(CimDocument {
                cim_namespaces: BTreeSet::from(["http://iec.ch/TC57/CIM100#".into()]),
                header: None,
                objects: vec![
                    CimObject {
                        class: "GeographicalRegion".into(),
                        id: "generated-region".into(),
                        definition: true,
                        props: Vec::new(),
                    },
                    CimObject {
                        class: "Substation".into(),
                        id: "substation".into(),
                        definition: true,
                        props: vec![
                            (
                                "Substation.Region".into(),
                                PropValue::Ref("generated-region".into()),
                            ),
                            (
                                "Substation.vendorProperty".into(),
                                PropValue::Text("retained".into()),
                            ),
                        ],
                    },
                    CimObject {
                        class: "ACLineSegment".into(),
                        id: "line".into(),
                        definition: true,
                        props: vec![(
                            "ACLineSegment.shortCircuitEndTemperature".into(),
                            PropValue::Text("80".into()),
                        )],
                    },
                    CimObject {
                        class: "LinearShuntCompensator".into(),
                        id: "shunt".into(),
                        definition: true,
                        props: vec![
                            (
                                "ShuntCompensator.nomU".into(),
                                PropValue::Text("230".into()),
                            ),
                            (
                                "ShuntCompensator.normalSections".into(),
                                PropValue::Text("1".into()),
                            ),
                        ],
                    },
                    CimObject {
                        class: "VendorControl".into(),
                        id: "control".into(),
                        definition: true,
                        props: Vec::new(),
                    },
                ],
            })
            .unwrap();

        let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
        warn_unmapped(&store, &mut warnings);
        assert!(warnings.iter().any(|warning| {
            warning.info.code == codes::READ_CGMES_FIELD_UNMAPPED.code
                && warning.contains("shortCircuitEndTemperature")
        }));
        assert!(warnings.iter().any(|warning| {
            warning.info.code == codes::READ_CGMES_FIELD_UNMAPPED.code
                && warning.contains("Substation.vendorProperty")
        }));
        assert!(warnings.iter().any(|warning| {
            warning.info.code == codes::READ_CGMES_RECORD_UNMAPPED.code
                && warning.contains("VendorControl")
        }));
        assert!(!warnings.iter().any(|warning| {
            warning.contains("GeographicalRegion")
                || warning.contains("Substation.Region")
                || warning.contains("ShuntCompensator.nomU")
                || warning.contains("ShuntCompensator.normalSections")
        }));
    }
}
