//! Coupled OPF numerical preparation. Ordinary AC-only builders keep refusing
//! equivalent owners; the private projection here accompanies the full DC and
//! station data and never mutates source declarations.
use crate::{AcOpfAssemblyOptions, AcOpfPreparation, Result, build_ac_opf_preparation};
use powerio_prob::{AcDcOpfInstance, AcDcQuantity, AcOpfInstance};
use powerio_tx::acdc::{AcDcConverterStation, AcDcLimits, DcEquivalentBranch, DcEquivalentBus};
use serde::Serialize;

/// Declared versus effective station parameters under the pinned reference.
#[derive(Clone, Debug, Serialize)]
pub struct AcDcStationPolicy {
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
/// DC branch declaration, endpoint power limit and pole-scaled conductance.
#[derive(Clone, Debug)]
pub struct AcDcBranchPreparation {
    pub source: DcEquivalentBranch,
    pub conductance_pu: f64,
    /// Zero is a zero limit, not the AC unlimited sentinel.
    pub active_power_limit_pu: f64,
}
/// Full coupled assembly: AC arrays and all equivalent DC equipment. A consumer
/// must assemble all three blocks. No solver tape or solved values live here.
#[derive(Clone, Debug)]
pub struct AcDcOpfPreparation {
    pub ac: AcOpfPreparation,
    pub base_mva: f64,
    pub pole_count: u8,
    pub dc_buses: Vec<AcDcBusPreparation>,
    pub dc_branches: Vec<AcDcBranchPreparation>,
    pub stations: Vec<AcDcStationPreparation>,
}
/// Apply the explicitly selected reference profile, preserving the input owner.
/// AC powers are pu and angle bounds use the standard PowerModels correction.
/// The profile uses LossCinv in both directions and widens Imax to the P/Q box
/// radius, recording both declarations and effective values.
// The instance keeps its validated network private; these lookups cannot fail.
#[allow(clippy::missing_panics_doc)]
pub fn build_ac_dc_opf_preparation(instance: &AcDcOpfInstance) -> Result<AcDcOpfPreparation> {
    let net = instance.network();
    let owner = net.ac_dc_equivalent().expect("validated instance owner");
    let mut projection = net.clone();
    projection.set_ac_dc_equivalent(None)?;
    let ac_instance = AcOpfInstance::from_network(projection)?
        .with_objective(powerio_prob::Objective::network_generator_cost());
    let mut ac = build_ac_opf_preparation(&ac_instance, &AcOpfAssemblyOptions::default())?;
    apply_initial_point(instance, &mut ac);
    if ac.generators.piecewise_linear.iter().any(Option::is_some) {
        return Err(crate::Error::UnsupportedOpfObjective {
            reason: "coupled reference profile currently qualifies polynomial costs only".into(),
        });
    }
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
        let policy = AcDcStationPolicy {
            component: s.component.to_string(),
            converter_base_kv: s.ac_base_kv,
            pcc_base_kv: pcc.base_kv,
            declared_current_limit_pu: s.valve_current_limit_pu,
            effective_current_limit_pu: s.valve_current_limit_pu.max(p.hypot(q)),
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
    let prepared = AcDcOpfPreparation {
        ac,
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
        dc_branches: owner
            .branches
            .iter()
            .filter(|b| b.in_service)
            .map(|b| AcDcBranchPreparation {
                source: b.clone(),
                conductance_pu: f64::from(owner.pole_count) / b.resistance_pu,
                active_power_limit_pu: b.ratings_mw[0] / owner.base_mva,
            })
            .collect(),
        stations,
    };
    validate_dc_parameters(&prepared)?;
    Ok(prepared)
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
