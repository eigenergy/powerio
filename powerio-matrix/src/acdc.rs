//! Coupled OPF numerical preparation. Ordinary AC-only builders keep refusing
//! equivalent owners; the private projection here accompanies the full DC and
//! station data and never mutates source declarations.
use crate::{AcOpfAssemblyOptions, AcOpfPreparation, Result, build_ac_opf_preparation};
use powerio_prob::{
    AcDcAngleReferencePolicy, AcDcCoordinatePolicy, AcDcCurrentLimitPolicy, AcDcOpfInstance,
    AcDcQuantity, AcOpfInstance,
};
use powerio_tx::acdc::{AcDcConverterStation, AcDcLimits, DcEquivalentBranch, DcEquivalentBus};
use serde::Serialize;

/// Explicit physical AC port and impedance bases, present only for the physical
/// profile. Voltages are line-to-line kV; impedances use owner MVA and converter side.
#[derive(Clone, Debug, Serialize)]
pub struct AcDcStationPhysicalBases {
    pub pcc_kv: f64,
    pub filter_kv: f64,
    pub valve_kv: f64,
    pub series_impedance_kv: f64,
    /// None for an ideal bypass, which ignores retained tap data.
    pub transformer_turns_ratio: Option<f64>,
}

/// Declared versus effective station parameters under the selected profile.
#[derive(Clone, Debug, Serialize)]
pub struct AcDcStationPolicy {
    pub coordinate_policy: AcDcCoordinatePolicy,
    pub physical_bases: Option<AcDcStationPhysicalBases>,
    /// Multiplies PCC pu voltage in the bypass equality, yielding converter pu.
    pub transformer_bypass_voltage_ratio: f64,
    pub current_limit_policy: AcDcCurrentLimitPolicy,
    pub component: String,
    pub converter_base_kv: f64,
    pub pcc_base_kv: f64,
    pub declared_current_limit_pu: f64,
    pub effective_current_limit_pu: f64,
    pub loss_coefficients_pu: [f64; 3],
    pub declared_rectifier_quadratic_ohm: f64,
    pub declared_inverter_quadratic_ohm: f64,
}
/// Source station plus effective OPF parameters. All bounds are in preparation
/// units (power pu on network MVA, angle radians, station AC current pu).
#[derive(Clone, Debug)]
pub struct AcDcStationPreparation {
    pub source: AcDcConverterStation,
    pub policy: AcDcStationPolicy,
    base_mva: f64,
}
impl AcDcStationPreparation {
    /// The reference auxiliary and valve bounds. Non-station quantities return
    /// None; PF control settings never become OPF equality constraints here.
    #[must_use]
    pub fn bounds(&self, quantity: AcDcQuantity) -> Option<AcDcLimits> {
        use AcDcQuantity::{
            FilterVoltageAngle, FilterVoltageMagnitude, PccActivePower, PccReactivePower,
            ReactorActivePowerFrom, ReactorReactivePowerFrom, StationDcActivePower,
            TransformerActivePowerTo, TransformerReactivePowerTo, ValveActivePower,
            ValveCurrentMagnitude, ValveReactivePower, ValveVoltageAngle, ValveVoltageMagnitude,
        };
        let s = &self.source;
        let p = s.valve_active_mw.min.abs().max(s.valve_active_mw.max.abs()) / self.base_mva;
        let q = s
            .valve_reactive_mvar
            .min
            .abs()
            .max(s.valve_reactive_mvar.max.abs())
            / self.base_mva;
        let (min, max) = match quantity {
            PccActivePower | TransformerActivePowerTo | ReactorActivePowerFrom => {
                (-2.0 * p, 2.0 * p)
            }
            PccReactivePower | TransformerReactivePowerTo | ReactorReactivePowerFrom => {
                (-2.0 * q, 2.0 * q)
            }
            ValveActivePower => (
                s.valve_active_mw.min / self.base_mva,
                s.valve_active_mw.max / self.base_mva,
            ),
            ValveReactivePower => (
                s.valve_reactive_mvar.min / self.base_mva,
                s.valve_reactive_mvar.max / self.base_mva,
            ),
            StationDcActivePower => (-1.2 * p, 1.2 * p),
            ValveCurrentMagnitude => (0.0, self.policy.effective_current_limit_pu),
            FilterVoltageMagnitude => (s.valve_voltage_pu.min / 1.2, s.valve_voltage_pu.max * 1.2),
            ValveVoltageMagnitude => (s.valve_voltage_pu.min, s.valve_voltage_pu.max),
            FilterVoltageAngle | ValveVoltageAngle => {
                (-std::f64::consts::TAU, std::f64::consts::TAU)
            }
            _ => return None,
        };
        Some(AcDcLimits { min, max })
    }
}
/// DC node declaration and power withdrawal in common-MVA pu.
#[derive(Clone, Debug)]
pub struct AcDcBusPreparation {
    pub source: DcEquivalentBus,
    pub withdrawal_pu: f64,
}
/// Branch coordinate provenance; DC voltage bases are per-pole kV.
#[derive(Clone, Debug, Serialize)]
pub struct AcDcBranchPolicy {
    pub component: String,
    pub coordinate_policy: AcDcCoordinatePolicy,
    pub from_base_kv: f64,
    pub to_base_kv: f64,
    /// Explicit physical resistance reference side; absent for source equations.
    pub resistance_base_kv: Option<f64>,
    /// Converts to-end pu voltage onto the from-end base (one for source profiles).
    pub to_voltage_scale: f64,
}

/// DC branch declaration, endpoint power limit and pole-scaled conductance.
#[derive(Clone, Debug)]
pub struct AcDcBranchPreparation {
    pub policy: AcDcBranchPolicy,
    pub source: DcEquivalentBranch,
    pub conductance_pu: f64,
    /// Zero is a zero limit, not the AC unlimited sentinel.
    pub active_power_limit_pu: f64,
}
/// A zero-radian numerical constraint with source-identity provenance.
/// A numerical anchor never promotes the source bus to a PF reference role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AcDcAngleReference {
    /// Bus whose AC voltage angle is fixed to zero.
    pub bus: powerio_tx::BusId,
    /// Active AC component, sorted by source bus ID; DC edges are excluded.
    pub island_buses: Vec<powerio_tx::BusId>,
    /// True only for a reference present in the source network.
    pub declared: bool,
}

/// Full coupled assembly: AC arrays and all equivalent DC equipment. A consumer
/// must assemble all three blocks. No solver tape or solved values live here.
#[derive(Clone, Debug)]
pub struct AcDcOpfPreparation {
    pub ac: AcOpfPreparation,
    /// All declared and added numerical angle references.
    pub angle_references: Vec<AcDcAngleReference>,
    pub base_mva: f64,
    pub pole_count: u8,
    pub dc_buses: Vec<AcDcBusPreparation>,
    pub dc_branches: Vec<AcDcBranchPreparation>,
    pub stations: Vec<AcDcStationPreparation>,
}
fn prepare_angle_references(
    instance: &AcDcOpfInstance,
    projection: &mut powerio_tx::BalancedNetwork,
) -> Result<Vec<AcDcAngleReference>> {
    let net = instance.network();
    let mut angle_references = Vec::new();
    for island in net.calc_islands().islands {
        let (references, declared) = if island.references.is_empty() {
            match instance.angle_reference_policy() {
                AcDcAngleReferencePolicy::Declared => (Vec::new(), false),
                AcDcAngleReferencePolicy::LowestBusIdPerIsland => {
                    (island.buses.first().copied().into_iter().collect(), false)
                }
                _ => {
                    return Err(crate::Error::UnsupportedOpfObjective {
                        reason: "unsupported coupled angle-reference policy".into(),
                    });
                }
            }
        } else {
            (island.references, true)
        };
        for bus in references {
            angle_references.push(AcDcAngleReference {
                bus,
                island_buses: island.buses.clone(),
                declared,
            });
            if !declared {
                // Private AC OPF projection only: its compiler represents angle
                // constraints through the reference set. No source/PF role changes.
                projection
                    .buses_mut()
                    .iter_mut()
                    .find(|b| b.id == bus)
                    .ok_or_else(|| crate::Error::UnsupportedOpfObjective {
                        reason: "angle-reference bus missing".into(),
                    })?
                    .kind = powerio_tx::BusType::Ref;
            }
        }
    }
    angle_references.sort_by_key(|r| r.bus);

    Ok(angle_references)
}

/// Apply the explicitly selected reference profile, preserving the input owner.
/// AC powers are pu and angle bounds use the standard PowerModels correction.
/// All profiles use LossCinv in both directions. Only the compatibility
/// profile widens Imax to the P/Q box radius; the declared-current profile
/// retains the source limit. Declarations and effective values are recorded.
// The instance keeps its validated network private; these lookups cannot fail.
#[allow(clippy::missing_panics_doc)]
pub fn build_ac_dc_opf_preparation(instance: &AcDcOpfInstance) -> Result<AcDcOpfPreparation> {
    let net = instance.network();
    let coordinate_policy = instance.profile().coordinate_policy();
    let physical = match coordinate_policy {
        AcDcCoordinatePolicy::SourcePerUnit => false,
        AcDcCoordinatePolicy::PhysicalLocalBases => true,
        _ => {
            return Err(crate::Error::UnsupportedOpfObjective {
                reason: "unsupported coupled coordinate policy".into(),
            });
        }
    };
    let owner = net.ac_dc_equivalent().expect("validated instance owner");
    let mut projection = net.clone();
    projection.set_ac_dc_equivalent(None)?;
    let angle_references = prepare_angle_references(instance, &mut projection)?;
    let ac_instance = AcOpfInstance::from_network(projection)?
        .with_objective(powerio_prob::Objective::network_generator_cost());
    let mut ac = build_ac_opf_preparation(&ac_instance, &AcOpfAssemblyOptions::default())?;
    apply_initial_point(instance, &mut ac);
    if ac.generators.piecewise_linear.iter().any(Option::is_some) {
        return Err(crate::Error::UnsupportedOpfObjective {
            reason: "coupled reference profile currently qualifies polynomial costs only".into(),
        });
    }
    let stations = prepare_stations(instance, coordinate_policy, physical)?;
    let mut dc_branches = Vec::new();
    for b in owner.branches.iter().filter(|b| b.in_service) {
        let from = owner
            .buses
            .iter()
            .find(|v| v.component == b.from)
            .expect("validated from endpoint");
        let to = owner
            .buses
            .iter()
            .find(|v| v.component == b.to)
            .expect("validated to endpoint");
        let scale = if physical {
            to.base_kv / from.base_kv
        } else {
            1.0
        };
        let conductance = f64::from(owner.pole_count) / b.resistance_pu;
        if physical
            && (!scale.is_finite()
                || scale <= 0.0
                || !scale.powi(2).is_finite()
                || scale.powi(2) <= 0.0
                || ![conductance * scale, conductance * scale.powi(2)]
                    .iter()
                    .all(|v| v.is_finite() && *v > 0.0))
        {
            return Err(crate::Error::UnsupportedOpfObjective {
                reason: "unrepresentable physical DC base ratio".into(),
            });
        }
        dc_branches.push(AcDcBranchPreparation {
            source: b.clone(),
            conductance_pu: conductance,
            active_power_limit_pu: b.ratings_mw[0] / owner.base_mva,
            policy: AcDcBranchPolicy {
                component: b.component.to_string(),
                coordinate_policy,
                from_base_kv: from.base_kv,
                to_base_kv: to.base_kv,
                resistance_base_kv: physical.then_some(from.base_kv),
                to_voltage_scale: scale,
            },
        });
    }
    let prepared = AcDcOpfPreparation {
        ac,
        angle_references,
        base_mva: owner.base_mva,
        pole_count: owner.pole_count,
        dc_buses: owner
            .buses
            .iter()
            .map(|b| AcDcBusPreparation {
                source: b.clone(),
                withdrawal_pu: b.withdrawal_mw / owner.base_mva,
            })
            .collect(),
        dc_branches,
        stations,
    };
    validate_dc_parameters(&prepared)?;
    Ok(prepared)
}

fn prepare_stations(
    instance: &AcDcOpfInstance,
    coordinate_policy: AcDcCoordinatePolicy,
    physical: bool,
) -> Result<Vec<AcDcStationPreparation>> {
    let net = instance.network();
    let owner = net.ac_dc_equivalent().expect("validated instance owner");
    let mut stations = Vec::new();
    for s in owner.stations.iter().filter(|s| s.in_service) {
        let pcc = net
            .buses()
            .iter()
            .find(|b| b.id == s.ac_bus)
            .expect("validated station PCC");
        let p = s.valve_active_mw.min.abs().max(s.valve_active_mw.max.abs()) / owner.base_mva;
        let q = s
            .valve_reactive_mvar
            .min
            .abs()
            .max(s.valve_reactive_mvar.max.abs())
            / owner.base_mva;
        let current_limit_policy = instance.profile().current_limit_policy();
        let effective_current_limit_pu = match current_limit_policy {
            AcDcCurrentLimitPolicy::PowerModelsAcDc0101Repair => {
                s.valve_current_limit_pu.max(p.hypot(q))
            }
            AcDcCurrentLimitPolicy::Declared => s.valve_current_limit_pu,
            _ => {
                return Err(crate::Error::UnsupportedOpfObjective {
                    reason: "unsupported coupled current-limit policy".into(),
                });
            }
        };
        let bypass_ratio = if physical {
            pcc.base_kv / s.ac_base_kv
        } else {
            1.0
        };
        let turns = physical
            .then_some(s.transformer_tap_ratio * bypass_ratio)
            .filter(|_| s.transformer.enabled);
        if !bypass_ratio.is_finite()
            || bypass_ratio <= 0.0
            || turns.is_some_and(|v| !v.is_finite() || v <= 0.0)
        {
            return Err(crate::Error::UnsupportedOpfObjective {
                reason: "unrepresentable physical station base ratio".into(),
            });
        }
        let policy = AcDcStationPolicy {
            coordinate_policy,
            transformer_bypass_voltage_ratio: bypass_ratio,
            physical_bases: physical.then_some(AcDcStationPhysicalBases {
                pcc_kv: pcc.base_kv,
                filter_kv: s.ac_base_kv,
                valve_kv: s.ac_base_kv,
                series_impedance_kv: s.ac_base_kv,
                transformer_turns_ratio: turns,
            }),
            current_limit_policy,
            component: s.component.to_string(),
            converter_base_kv: s.ac_base_kv,
            pcc_base_kv: pcc.base_kv,
            declared_current_limit_pu: s.valve_current_limit_pu,
            effective_current_limit_pu,
            loss_coefficients_pu: [
                s.losses.constant_mw / owner.base_mva,
                s.losses.linear_kv / (3.0_f64.sqrt() * s.ac_base_kv),
                s.losses.inverter_quadratic_ohm * owner.base_mva / (3.0 * s.ac_base_kv.powi(2)),
            ],
            declared_rectifier_quadratic_ohm: s.losses.rectifier_quadratic_ohm,
            declared_inverter_quadratic_ohm: s.losses.inverter_quadratic_ohm,
        };
        if policy.loss_coefficients_pu.iter().any(|v| !v.is_finite())
            || !policy.effective_current_limit_pu.is_finite()
        {
            return Err(powerio_core::Error::new(
                &powerio_prob::diagnostics::codes::BUILD_ACDC_PROFILE_INVALID,
                "nonfinite effective station parameters",
            )
            .into());
        }
        stations.push(AcDcStationPreparation {
            source: s.clone(),
            policy,
            base_mva: owner.base_mva,
        });
    }
    Ok(stations)
}

fn apply_initial_point(instance: &AcDcOpfInstance, ac: &mut AcOpfPreparation) {
    if let Some(point) = instance.initial_point() {
        for v in point.values() {
            use AcDcQuantity::{
                AcVoltageAngle, AcVoltageMagnitude, GeneratorActivePower, GeneratorReactivePower,
            };
            match v.quantity {
                AcVoltageMagnitude | AcVoltageAngle => {
                    let row = ac
                        .bus_ids
                        .iter()
                        .position(|id| id.0.to_string() == v.identity)
                        .expect("validated AC identity");
                    if v.quantity == AcVoltageMagnitude {
                        ac.buses.initial_vm[row] = v.value;
                    } else {
                        ac.buses.initial_va[row] = v.value;
                    }
                }
                GeneratorActivePower | GeneratorReactivePower => {
                    let row = ac
                        .generators
                        .identities
                        .iter()
                        .position(|id| *id == v.identity)
                        .expect("validated generator identity");
                    if v.quantity == GeneratorActivePower {
                        ac.generators.pg[row] = v.value / instance.network().base_mva();
                    } else {
                        ac.generators.qg[row] = v.value / instance.network().base_mva();
                    }
                }
                _ => {}
            }
        }
    }
}

fn validate_dc_parameters(p: &AcDcOpfPreparation) -> Result<()> {
    if p.dc_buses.iter().any(|b| !b.withdrawal_pu.is_finite())
        || p.dc_branches
            .iter()
            .any(|b| !b.conductance_pu.is_finite() || !b.active_power_limit_pu.is_finite())
    {
        return Err(powerio_core::Error::new(
            &powerio_prob::diagnostics::codes::BUILD_ACDC_PROFILE_INVALID,
            "nonfinite effective DC parameters",
        )
        .into());
    }
    Ok(())
}
