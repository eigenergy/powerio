//! Coupled balanced AC/DC OPF intent and identity-keyed electrical results.
//! This profile selects source-coordinate ACP VSC equations, network generator
//! cost and all operating limits, with free OPF dispatch (no PF setpoints).
use crate::diagnostics::codes;
use crate::{Producer, Termination};
use powerio_core::Error;
use powerio_tx::{BalancedNetwork, BusType, acdc::ConverterTechnology};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// An explicit, versioned equation and bound convention; no implicit default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AcDcOpfProfile {
    /// PowerModelsACDC 0.10.1 at 9ada4082: source pu coordinates, inverter
    /// quadratic loss in both directions, repaired Imax and auxiliary bounds.
    PowerModelsAcDc0101AcpVscV1,
    /// The same ACP VSC equations, inverter losses and auxiliary bounds, but
    /// enforce declared valve-current limits without the nominal-1-pu repair.
    /// This qualifies current-limit semantics, not arbitrary physical rebasing.
    DeclaredCurrentAcpVscV1,
}

/// Valve-current bound semantics fixed by the selected OPF profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AcDcCurrentLimitPolicy {
    /// Raise Imax_pu to at least the P/Q rating-box radius on a nominal 1-pu voltage.
    PowerModelsAcDc0101Repair,
    /// Honour the declared AC RMS current bound on S/(sqrt(3)*V_converter).
    /// P/Q ratings do not authorize an increase; infeasibility is retained.
    Declared,
}
impl AcDcOpfProfile {
    #[must_use]
    pub const fn current_limit_policy(self) -> AcDcCurrentLimitPolicy {
        match self {
            Self::PowerModelsAcDc0101AcpVscV1 => AcDcCurrentLimitPolicy::PowerModelsAcDc0101Repair,
            Self::DeclaredCurrentAcpVscV1 => AcDcCurrentLimitPolicy::Declared,
        }
    }
}

/// Numerical angle constraints for coupled OPF, independent of source bus roles.
/// This does not choose PF voltage or generation controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AcDcAngleReferencePolicy {
    /// Preserve all declared references; preparation refuses uncovered islands.
    #[default]
    Declared,
    /// Preserve declared references. In each active AC island without one,
    /// fix the lowest source bus ID's angle to zero. DC links do not join islands.
    LowestBusIdPerIsland,
}
impl AcDcAngleReferencePolicy {
    /// Whether serialization may omit this backward-compatible default.
    #[must_use]
    pub const fn is_declared(&self) -> bool {
        matches!(self, Self::Declared)
    }
}

/// Electrical quantities, independent of solver variable names and ordering.
/// Bus identities are decimal BusId; all other identities are persistent UIDs.
/// Powers are positive into equipment terminals, except generator production.
/// Valve power is positive from the AC reactor into the valve; DC power enters
/// the station from the DC bus. Current pu uses the station AC voltage base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AcDcQuantity {
    /// ac_bus quantity, pu.
    AcVoltageMagnitude,
    /// ac_bus quantity, rad.
    AcVoltageAngle,
    /// generator quantity, MW.
    GeneratorActivePower,
    /// generator quantity, MVAr.
    GeneratorReactivePower,
    /// ac_branch quantity, MW.
    AcBranchActivePowerFrom,
    /// ac_branch quantity, MVAr.
    AcBranchReactivePowerFrom,
    /// ac_branch quantity, MW.
    AcBranchActivePowerTo,
    /// ac_branch quantity, MVAr.
    AcBranchReactivePowerTo,
    /// dc_bus quantity, pu.
    DcVoltageMagnitude,
    /// dc_branch quantity, MW.
    DcBranchActivePowerFrom,
    /// dc_branch quantity, MW.
    DcBranchActivePowerTo,
    /// station quantity, MW.
    PccActivePower,
    /// station quantity, MVAr.
    PccReactivePower,
    /// station quantity, MW.
    TransformerActivePowerTo,
    /// station quantity, MVAr.
    TransformerReactivePowerTo,
    /// station quantity, MW.
    ReactorActivePowerFrom,
    /// station quantity, MVAr.
    ReactorReactivePowerFrom,
    /// station quantity, MW.
    ValveActivePower,
    /// station quantity, MVAr.
    ValveReactivePower,
    /// station quantity, MW.
    StationDcActivePower,
    /// station quantity, pu.
    ValveCurrentMagnitude,
    /// station quantity, pu.
    FilterVoltageMagnitude,
    /// station quantity, rad.
    FilterVoltageAngle,
    /// station quantity, pu.
    ValveVoltageMagnitude,
    /// station quantity, rad.
    ValveVoltageAngle,
}
impl AcDcQuantity {
    /// Unit of the public value. Numerical preparation scales powers to pu.
    #[must_use]
    pub const fn unit(self) -> &'static str {
        match self {
            Self::AcVoltageMagnitude
            | Self::DcVoltageMagnitude
            | Self::ValveCurrentMagnitude
            | Self::FilterVoltageMagnitude
            | Self::ValveVoltageMagnitude => "pu",
            Self::AcVoltageAngle | Self::FilterVoltageAngle | Self::ValveVoltageAngle => "rad",
            Self::GeneratorActivePower
            | Self::AcBranchActivePowerFrom
            | Self::AcBranchActivePowerTo
            | Self::DcBranchActivePowerFrom
            | Self::DcBranchActivePowerTo
            | Self::PccActivePower
            | Self::TransformerActivePowerTo
            | Self::ReactorActivePowerFrom
            | Self::ValveActivePower
            | Self::StationDcActivePower => "MW",
            Self::GeneratorReactivePower
            | Self::AcBranchReactivePowerFrom
            | Self::AcBranchReactivePowerTo
            | Self::PccReactivePower
            | Self::TransformerReactivePowerTo
            | Self::ReactorReactivePowerFrom
            | Self::ValveReactivePower => "MVAr",
        }
    }
}

/// One identity-keyed scalar, never a solver row or column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcDcValue {
    pub quantity: AcDcQuantity,
    pub identity: String,
    pub value: f64,
}

/// A complete electrical point for this coupled profile. Validation is against
/// the owning instance, including active equipment and every required quantity.
#[derive(Clone, Debug, PartialEq)]
pub struct AcDcOperatingPoint {
    values: Vec<AcDcValue>,
}
impl AcDcOperatingPoint {
    /// Validate an exact identity/quantity set and finite values. This checks
    /// shape, not equation feasibility; starts may be infeasible.
    pub fn new(instance: &AcDcOpfInstance, values: Vec<AcDcValue>) -> Result<Self, Error> {
        let expected = instance.coordinates();
        let mut seen = BTreeSet::new();
        for v in &values {
            let key = (v.quantity, v.identity.clone());
            if !v.value.is_finite() || !expected.contains(&key) || !seen.insert(key) {
                return Err(invalid(
                    "nonfinite, duplicate or unknown AC/DC point coordinate",
                ));
            }
        }
        if seen != expected {
            return Err(invalid("incomplete AC/DC operating point"));
        }
        Ok(Self { values })
    }
    #[must_use]
    pub fn values(&self) -> &[AcDcValue] {
        &self.values
    }
}

/// Matrix-free coupled OPF intent. The versioned profile fixes objective,
/// dispatch freedom and active limits. The source owner is retained unchanged.
#[derive(Clone, Debug)]
pub struct AcDcOpfInstance {
    network: BalancedNetwork,
    profile: AcDcOpfProfile,
    angle_reference_policy: AcDcAngleReferencePolicy,
    initial_point: Option<AcDcOperatingPoint>,
}
impl AcDcOpfInstance {
    /// Select the profile explicitly. Unsupported equipment is refused rather
    /// than converted into an AC-only optimization.
    // Base identity is exact: a tolerance would silently rebase source coordinates.
    #[allow(clippy::float_cmp)]
    pub fn from_network(
        mut network: BalancedNetwork,
        profile: AcDcOpfProfile,
    ) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        network.validate().map_err(|e| invalid(e.to_string()))?;
        for ids in [
            network
                .generators()
                .iter()
                .map(|v| v.uid.as_deref().unwrap_or(""))
                .collect::<Vec<_>>(),
            network
                .branches()
                .iter()
                .map(|v| v.uid.as_deref().unwrap_or(""))
                .collect(),
        ] {
            if ids.iter().any(|id| id.is_empty())
                || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
            {
                return Err(invalid("duplicate or empty AC component identity"));
            }
        }
        let owner = network
            .ac_dc_equivalent()
            .ok_or_else(|| invalid("AC/DC equivalent owner missing"))?;
        owner
            .validate(&network)
            .map_err(|e| invalid(e.to_string()))?;
        if network.is_normalized() || owner.base_mva != network.base_mva() {
            return Err(invalid(
                "AC/DC profile requires unnormalized AC data on the owner MVA base",
            ));
        }
        if network.hvdc().iter().any(|v| v.in_service)
            || network.detailed_connectivity().as_ref().is_some_and(|d| {
                !d.voltage_source_converters.is_empty()
                    || !d.line_commutated_converters.is_empty()
                    || !d.dc_lines.is_empty()
            })
        {
            return Err(invalid(
                "mixed physical/aggregate/equivalent DC equipment is unsupported",
            ));
        }
        // Expanded three-winding and storage decisions require a separately
        // qualified coordinate/result interface, not a silent projection.
        if network
            .branches()
            .iter()
            .any(|b| b.in_service && b.from == b.to)
        {
            return Err(invalid(
                "AC/DC terminal result profile does not support AC self loops",
            ));
        }
        if !network.transformers_3w().is_empty() || !network.storage().is_empty() {
            return Err(invalid(
                "AC/DC profile does not yet support storage or three-winding transformers",
            ));
        }
        if owner
            .stations
            .iter()
            .any(|s| s.in_service && s.technology != ConverterTechnology::Vsc)
        {
            return Err(invalid("AC/DC profile qualifies VSC only"));
        }
        if owner
            .branches
            .iter()
            .any(|b| b.in_service && b.resistance_pu <= 0.0)
        {
            return Err(invalid(
                "AC/DC profile requires positive DC branch resistance",
            ));
        }
        for s in owner.stations.iter().filter(|s| s.in_service) {
            if !network
                .buses()
                .iter()
                .any(|b| b.id == s.ac_bus && b.kind != BusType::Isolated)
            {
                return Err(invalid("active station has no active AC PCC"));
            }
            for series in [&s.transformer, &s.reactor] {
                if series.enabled && series.resistance_pu.hypot(series.reactance_pu) == 0.0 {
                    return Err(invalid("enabled station series impedance is zero"));
                }
            }
        }
        Ok(Self {
            network,
            profile,
            angle_reference_policy: AcDcAngleReferencePolicy::Declared,
            initial_point: None,
        })
    }
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }
    #[must_use]
    pub const fn profile(&self) -> AcDcOpfProfile {
        self.profile
    }
    /// Select numerical angle references without changing any source bus kind.
    #[must_use]
    pub const fn with_angle_reference_policy(mut self, policy: AcDcAngleReferencePolicy) -> Self {
        self.angle_reference_policy = policy;
        self
    }
    #[must_use]
    pub const fn angle_reference_policy(&self) -> AcDcAngleReferencePolicy {
        self.angle_reference_policy
    }
    #[must_use]
    pub const fn initial_point(&self) -> Option<&AcDcOperatingPoint> {
        self.initial_point.as_ref()
    }
    /// Revalidate the point against this instance; attaching it does not add PF constraints.
    pub fn with_initial_point(mut self, point: AcDcOperatingPoint) -> Result<Self, Error> {
        self.initial_point = Some(AcDcOperatingPoint::new(&self, point.values)?);
        Ok(self)
    }
    /// Complete electrical coordinate set, stable under row reordering.
    #[must_use]
    // Private construction guarantees the owner and assigned identities.
    #[allow(clippy::missing_panics_doc)]
    pub fn coordinates(&self) -> BTreeSet<(AcDcQuantity, String)> {
        use AcDcQuantity::{
            AcBranchActivePowerFrom, AcBranchActivePowerTo, AcBranchReactivePowerFrom,
            AcBranchReactivePowerTo, AcVoltageAngle, AcVoltageMagnitude, DcBranchActivePowerFrom,
            DcBranchActivePowerTo, DcVoltageMagnitude, FilterVoltageAngle, FilterVoltageMagnitude,
            GeneratorActivePower, GeneratorReactivePower, PccActivePower, PccReactivePower,
            ReactorActivePowerFrom, ReactorReactivePowerFrom, StationDcActivePower,
            TransformerActivePowerTo, TransformerReactivePowerTo, ValveActivePower,
            ValveCurrentMagnitude, ValveReactivePower, ValveVoltageAngle, ValveVoltageMagnitude,
        };
        let net = &self.network;
        let owner = net.ac_dc_equivalent().expect("validated owner");
        let mut keys = BTreeSet::new();
        let mut add = |id: String, qs: &[AcDcQuantity]| {
            for &q in qs {
                keys.insert((q, id.clone()));
            }
        };
        let active_buses: BTreeSet<_> = net
            .buses()
            .iter()
            .filter(|b| b.kind != BusType::Isolated)
            .map(|b| b.id)
            .collect();
        let active = |id| active_buses.contains(&id);
        for b in net.buses().iter().filter(|b| b.kind != BusType::Isolated) {
            add(b.id.0.to_string(), &[AcVoltageMagnitude, AcVoltageAngle]);
        }
        for g in net
            .generators()
            .iter()
            .filter(|g| g.in_service && active(g.bus))
        {
            add(
                g.uid.clone().expect("validated assigned identity"),
                &[GeneratorActivePower, GeneratorReactivePower],
            );
        }
        for b in net
            .branches()
            .iter()
            .filter(|b| b.in_service && active(b.from) && active(b.to))
        {
            add(
                b.uid.clone().expect("validated assigned identity"),
                &[
                    AcBranchActivePowerFrom,
                    AcBranchReactivePowerFrom,
                    AcBranchActivePowerTo,
                    AcBranchReactivePowerTo,
                ],
            );
        }
        for b in &owner.buses {
            add(b.component.to_string(), &[DcVoltageMagnitude]);
        }
        for b in owner.branches.iter().filter(|b| b.in_service) {
            add(
                b.component.to_string(),
                &[DcBranchActivePowerFrom, DcBranchActivePowerTo],
            );
        }
        for s in owner.stations.iter().filter(|s| s.in_service) {
            add(
                s.component.to_string(),
                &[
                    PccActivePower,
                    PccReactivePower,
                    TransformerActivePowerTo,
                    TransformerReactivePowerTo,
                    ReactorActivePowerFrom,
                    ReactorReactivePowerFrom,
                    ValveActivePower,
                    ValveReactivePower,
                    StationDcActivePower,
                    ValveCurrentMagnitude,
                    FilterVoltageMagnitude,
                    FilterVoltageAngle,
                    ValveVoltageMagnitude,
                    ValveVoltageAngle,
                ],
            );
        }
        keys
    }
}

/// A local primal OPF result. Converged never asserts global optimality;
/// unqualified duals and solver tapes are deliberately absent.
#[derive(Clone, Debug)]
pub struct AcDcOpfSolution {
    instance: Arc<AcDcOpfInstance>,
    point: AcDcOperatingPoint,
    objective: f64,
    termination: Termination,
    producer: Producer,
}
impl AcDcOpfSolution {
    pub fn new(
        instance: Arc<AcDcOpfInstance>,
        point: AcDcOperatingPoint,
        objective: f64,
        termination: Termination,
        producer: Producer,
    ) -> Result<Self, Error> {
        if !objective.is_finite() {
            return Err(invalid("nonfinite AC/DC objective"));
        }
        let point = AcDcOperatingPoint::new(&instance, point.values)?;
        Ok(Self {
            instance,
            point,
            objective,
            termination,
            producer,
        })
    }
    #[must_use]
    pub fn instance(&self) -> &AcDcOpfInstance {
        &self.instance
    }
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        self.instance.network()
    }
    #[must_use]
    pub const fn point(&self) -> &AcDcOperatingPoint {
        &self.point
    }
    #[must_use]
    pub const fn objective(&self) -> f64 {
        self.objective
    }
    #[must_use]
    pub const fn termination(&self) -> &Termination {
        &self.termination
    }
    #[must_use]
    pub const fn producer(&self) -> &Producer {
        &self.producer
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(&codes::BUILD_ACDC_PROFILE_INVALID, message)
}
