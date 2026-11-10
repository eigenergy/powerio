//! Explicit coupled PF intent, distinct from OPF dispatch and operating limits.
use crate::{
    AcDcCoordinatePolicy, AcDcOperatingPoint, AcDcOpfInstance, AcDcOpfProfile, AcDcQuantity,
    AcDcStationLossMode, AcDcValue, Producer, Termination,
};
use powerio_core::Error;
use powerio_tx::{
    BalancedNetwork, BusType,
    acdc::{AcConverterControl, DcConverterControl},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

/// A versioned PF control and allocation contract; never inferred from OPF.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AcDcPfProfile {
    /// PMACDC 0.10.1 source-pu/Cinv equations and bus-role repair. Generator
    /// voltage targets use bus Vm. Non-reference generators retain Pg; PV and
    /// reference generators share changes from declared Qg equally. Reference
    /// generators also share changes from declared Pg equally. Exactly one AC
    /// reference per AC island and one DC voltage controller per DC island.
    /// Converter P/Q controls use PCC withdrawals; AC voltage uses bus Vm and
    /// DC voltage uses converter Vdcset. Overlapping AC voltage controls refuse.
    /// Limits are reported after PF, without clipping or PV/PQ switching.
    PowerModelsAcDc0101AcpVscEqualIncrementV1,
    /// Same electrical/P-reference semantics, allowing shared AC voltage authority.
    /// Every active generator and VoltageAtPcc converter at a bus shares equal
    /// reactive INJECTION increments from declarations: delta Qg = -delta Qpcc.
    /// All participants use one bus Vm target. Converter-only participants share
    /// equal withdrawal increments. ReactivePowerAtPcc converters do not participate.
    /// No rating-based weights, saturation, source-control edits or PV/PQ switching.
    PowerModelsAcDc0101AcpVscSharedVoltageEqualIncrementV1,
    /// Shared AC-voltage allocation with explicit DC-power droop (type 3).
    /// Withdrawal-positive Pdc/base = Pdcset/base - (Vdc - Vdcset)/droop.
    /// Active droop slopes and targets must be positive, with a finite reciprocal
    /// slope and zero deadband. Each DC island needs at least one fixed-voltage
    /// or droop controller, and at most one fixed-voltage controller. Type-4
    /// valve-power droop is unsupported. Limits are reported without saturation
    /// or control switching; inactive and unused declarations remain retained.
    PowerModelsAcDc0101AcpVscDcDroopSharedVoltageEqualIncrementV1,
    /// Physical local voltage/impedance bases, Cinv losses and declared current
    /// ratings, with DC-power droop and shared reactive injection increments.
    /// PCC voltage uses the AC bus base, filter/valve voltage the station base,
    /// and DC voltage its local per-pole base. Droop k = R*S/Vdc_base, where
    /// R is per-pole kV per total station MW; Vdcset is on that DC bus base.
    /// This explicitly interprets declarations; it does not automatically rebase
    /// source data. Limits remain post-solve reports without saturation.
    PhysicalCoordinatesDcDroopSharedVoltageEqualIncrementV1,
    /// Physical-coordinate droop/shared-voltage PF with one explicit loss mode
    /// per active station. Rectifier/inverter select Crec/Cinv and require valve
    /// AC withdrawal P >= 0 / <= 0. Zero P retains the selected curve. Mode sign
    /// is a model-domain requirement; operating ratings remain post-solve reports.
    /// No automatic reversal, saturation or control switching.
    FixedModePhysicalDcDroopSharedVoltageEqualIncrementV1,
}

impl AcDcPfProfile {
    #[must_use]
    pub const fn shares_voltage_reactive_increments(self) -> bool {
        matches!(
            self,
            Self::PowerModelsAcDc0101AcpVscSharedVoltageEqualIncrementV1
                | Self::PowerModelsAcDc0101AcpVscDcDroopSharedVoltageEqualIncrementV1
                | Self::PhysicalCoordinatesDcDroopSharedVoltageEqualIncrementV1
                | Self::FixedModePhysicalDcDroopSharedVoltageEqualIncrementV1
        )
    }
    #[must_use]
    pub const fn allows_dc_power_droop(self) -> bool {
        matches!(
            self,
            Self::PowerModelsAcDc0101AcpVscDcDroopSharedVoltageEqualIncrementV1
                | Self::PhysicalCoordinatesDcDroopSharedVoltageEqualIncrementV1
                | Self::FixedModePhysicalDcDroopSharedVoltageEqualIncrementV1
        )
    }
    #[must_use]
    pub const fn coordinate_policy(self) -> AcDcCoordinatePolicy {
        match self {
            Self::PhysicalCoordinatesDcDroopSharedVoltageEqualIncrementV1
            | Self::FixedModePhysicalDcDroopSharedVoltageEqualIncrementV1 => {
                AcDcCoordinatePolicy::PhysicalLocalBases
            }
            _ => AcDcCoordinatePolicy::SourcePerUnit,
        }
    }
}

/// Source equipment and explicit PF semantics. Shared electrical validation
/// does not enable OPF bounds or generator costs as PF requirements.
#[derive(Clone, Debug)]
pub struct AcDcPfInstance {
    electrical: AcDcOpfInstance,
    profile: AcDcPfProfile,
}
impl AcDcPfInstance {
    pub fn from_network(network: BalancedNetwork, profile: AcDcPfProfile) -> Result<Self, Error> {
        Self::from_network_with_loss_modes(network, profile, Vec::new())
    }
    /// Complete active-station loss modes are required only by fixed-mode PF.
    /// Existing profiles reject nonempty modes. Missing, duplicate, inactive,
    /// unknown or sign/rating-incompatible selections refuse before solving.
    pub fn from_network_with_loss_modes(
        network: BalancedNetwork,
        profile: AcDcPfProfile,
        loss_modes: Vec<AcDcStationLossMode>,
    ) -> Result<Self, Error> {
        let electrical_profile = if profile
            == AcDcPfProfile::FixedModePhysicalDcDroopSharedVoltageEqualIncrementV1
        {
            AcDcOpfProfile::FixedModePhysicalAcpVscV1
        } else {
            match profile.coordinate_policy() {
                AcDcCoordinatePolicy::PhysicalLocalBases => {
                    AcDcOpfProfile::PhysicalCoordinatesAcpVscV1
                }
                AcDcCoordinatePolicy::SourcePerUnit => AcDcOpfProfile::PowerModelsAcDc0101AcpVscV1,
            }
        };
        let electrical =
            AcDcOpfInstance::from_network_with_loss_modes(network, electrical_profile, loss_modes)?;
        validate_controls(electrical.network(), profile)?;
        Ok(Self {
            electrical,
            profile,
        })
    }
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        self.electrical.network()
    }
    #[must_use]
    pub const fn profile(&self) -> AcDcPfProfile {
        self.profile
    }
    #[must_use]
    pub fn loss_modes(&self) -> &[AcDcStationLossMode] {
        self.electrical.loss_modes()
    }
    #[must_use]
    pub fn coordinates(&self) -> BTreeSet<(AcDcQuantity, String)> {
        self.electrical.coordinates()
    }
    #[must_use]
    pub const fn initial_point(&self) -> Option<&AcDcOperatingPoint> {
        self.electrical.initial_point()
    }
    pub fn with_initial_point(mut self, point: AcDcOperatingPoint) -> Result<Self, Error> {
        self.electrical = self.electrical.with_initial_point(point)?;
        Ok(self)
    }
}
impl AcDcOperatingPoint {
    /// Validate a complete, finite PF point; this does not assert convergence.
    pub fn for_pf(instance: &AcDcPfInstance, values: Vec<AcDcValue>) -> Result<Self, Error> {
        Self::new(&instance.electrical, values)
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(
        &crate::diagnostics::codes::BUILD_ACDC_PROFILE_INVALID,
        message,
    )
}
fn components(mut nodes: BTreeSet<String>, edges: &[(String, String)]) -> Vec<BTreeSet<String>> {
    let mut result = Vec::new();
    while let Some(root) = nodes.pop_first() {
        let mut group = BTreeSet::from([root.clone()]);
        let mut queue = vec![root];
        while let Some(v) = queue.pop() {
            for (f, t) in edges {
                let other = if f == &v {
                    t
                } else if t == &v {
                    f
                } else {
                    continue;
                };
                if nodes.remove(other) {
                    group.insert(other.clone());
                    queue.push(other.clone());
                }
            }
        }
        result.push(group);
    }
    result
}
fn validate_controls(net: &BalancedNetwork, profile: AcDcPfProfile) -> Result<(), Error> {
    let owner = net
        .ac_dc_equivalent()
        .ok_or_else(|| invalid("AC/DC owner missing"))?;
    let active: BTreeSet<_> = net
        .buses()
        .iter()
        .filter(|b| b.kind != BusType::Isolated)
        .map(|b| b.id)
        .collect();
    let gen_buses: BTreeSet<_> = net
        .generators()
        .iter()
        .filter(|g| g.in_service && active.contains(&g.bus))
        .map(|g| g.bus)
        .collect();
    for b in net.buses().iter().filter(|b| active.contains(&b.id)) {
        if b.kind == BusType::Ref && !gen_buses.contains(&b.id) {
            return Err(invalid("PF reference needs active generation"));
        }
        if (gen_buses.contains(&b.id) || b.kind == BusType::Ref) && b.vm <= 0. {
            return Err(invalid("PF AC voltage target must be positive"));
        }
    }
    let indexed = powerio_tx::IndexedNetwork::new(net);
    let edges: Vec<_> = net
        .branches()
        .iter()
        .filter(|b| b.in_service)
        .map(|b| (b.from, b.to))
        .chain(indexed.closed_switches().map(|(_, s)| (s.from, s.to)))
        .filter(|(f, t)| active.contains(f) && active.contains(t))
        .map(|(f, t)| (f.0.to_string(), t.0.to_string()))
        .collect();
    for island in components(active.iter().map(|b| b.0.to_string()).collect(), &edges) {
        if net
            .buses()
            .iter()
            .filter(|b| b.kind == BusType::Ref && island.contains(&b.id.0.to_string()))
            .count()
            != 1
        {
            return Err(invalid(
                "PF requires exactly one declared AC reference per AC island",
            ));
        }
    }
    let mut voltage_buses = gen_buses;
    for s in owner.stations.iter().filter(|s| s.in_service) {
        let droop =
            s.dc_control == DcConverterControl::DcPowerDroop && profile.allows_dc_power_droop();
        if !matches!(
            s.dc_control,
            DcConverterControl::ActivePowerAtPcc | DcConverterControl::DcVoltage
        ) && !droop
        {
            return Err(invalid("PF droop controls are unqualified"));
        }
        if (s.dc_control == DcConverterControl::DcVoltage || droop)
            && s.settings.dc_voltage_target_pu <= 0.
        {
            return Err(invalid("PF DC voltage target must be positive"));
        }
        if droop {
            if s.settings.droop <= 0. || !s.settings.droop.recip().is_finite() {
                return Err(invalid(
                    "PF DC droop slope must be positive with finite reciprocal",
                ));
            }
            if s.settings.dc_voltage_deadband_pu != 0. {
                return Err(invalid("PF DC droop deadband is unqualified"));
            }
        }
        if s.ac_control == AcConverterControl::VoltageAtPcc {
            if !voltage_buses.insert(s.ac_bus) && !profile.shares_voltage_reactive_increments() {
                return Err(invalid(
                    "PF overlapping AC voltage controllers require an explicit sharing model",
                ));
            }
            if !net.buses().iter().any(|b| b.id == s.ac_bus && b.vm > 0.) {
                return Err(invalid("PF AC voltage target must be positive"));
            }
        }
    }
    validate_dc_authority(owner, profile)
}
fn validate_dc_authority(
    owner: &powerio_tx::acdc::BalancedAcDcEquipment,
    profile: AcDcPfProfile,
) -> Result<(), Error> {
    let edges: Vec<_> = owner
        .branches
        .iter()
        .filter(|b| b.in_service)
        .map(|b| (b.from.to_string(), b.to.to_string()))
        .collect();
    for island in components(
        owner
            .buses
            .iter()
            .map(|b| b.component.to_string())
            .collect(),
        &edges,
    ) {
        let stations: Vec<_> = owner
            .stations
            .iter()
            .filter(|s| s.in_service && island.contains(&s.dc_bus.to_string()))
            .collect();
        let fixed = stations
            .iter()
            .filter(|s| s.dc_control == DcConverterControl::DcVoltage)
            .count();
        if profile.allows_dc_power_droop() {
            let droop = stations
                .iter()
                .filter(|s| s.dc_control == DcConverterControl::DcPowerDroop)
                .count();
            if fixed > 1 || fixed + droop == 0 {
                return Err(invalid(
                    "PF DC island needs at least one fixed-voltage or positive-droop controller and at most one fixed-voltage controller",
                ));
            }
        } else if fixed != 1 {
            return Err(invalid(
                "PF requires exactly one declared DC voltage controller per DC island",
            ));
        }
    }
    Ok(())
}

/// A converged electrical PF point need not respect equipment operating limits.
/// Those are solver validation reports, not dispatch constraints or saturation.
#[derive(Clone, Debug)]
pub struct AcDcPfSolution {
    instance: Arc<AcDcPfInstance>,
    point: AcDcOperatingPoint,
    termination: Termination,
    producer: Producer,
}
impl AcDcPfSolution {
    pub fn new(
        instance: Arc<AcDcPfInstance>,
        point: AcDcOperatingPoint,
        termination: Termination,
        producer: Producer,
    ) -> Result<Self, Error> {
        let point = AcDcOperatingPoint::for_pf(&instance, point.into_values())?;
        Ok(Self {
            instance,
            point,
            termination,
            producer,
        })
    }
    #[must_use]
    pub fn instance(&self) -> &AcDcPfInstance {
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
    pub const fn termination(&self) -> &Termination {
        &self.termination
    }
    #[must_use]
    pub const fn producer(&self) -> &Producer {
        &self.producer
    }
}
