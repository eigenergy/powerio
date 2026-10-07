//! Ideal positive/negative-sequence boundary with an explicit zero-sequence
//! circuit. Finite positive-sequence impedance still requires separate mapping.

use std::f64::consts::TAU;

use num_complex::Complex64;

use super::{
    format_error,
    infeeder::{
        InfeederInput, InfeederSetpoint, InternalImpedance, SourceGrounding, SourceZeroSequence,
    },
    load_mapping::resolve_voltage,
    semantics::{Connection, State},
};
use crate::{DistBus, DistShunt, DistSwitch, Result, VoltageSource};

pub(super) struct SourceCircuit {
    pub bus: DistBus,
    pub source: VoltageSource,
    pub switch: DistSwitch,
}

/// Internal circuit data, not a serializable network or a public ABI view.
/// A terminal reference is on the device-local bus; it is not an earth
/// connection and does not connect to an external neutral by coincidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum VoltageReference {
    Earth,
    Terminal(String),
}

pub(super) struct IdealVoltageBoundary {
    pub name: String,
    pub bus: String,
    pub terminals: [String; 3],
    pub reference: VoltageReference,
    pub magnitudes: [f64; 3],
    pub angles: [f64; 3],
}

/// V(positive) - V(negative) = voltage. None denotes earth. The transpose
/// of these incidence coefficients carries source current into nodal KCL;
/// omitting its negative column would silently ground a floating source.
pub(super) struct VoltageEquation {
    pub positive: String,
    pub negative: Option<String>,
    pub voltage: Complex64,
}

impl IdealVoltageBoundary {
    pub fn equations(&self) -> [VoltageEquation; 3] {
        std::array::from_fn(|index| VoltageEquation {
            positive: self.terminals[index].clone(),
            negative: match &self.reference {
                VoltageReference::Earth => None,
                VoltageReference::Terminal(terminal) => Some(terminal.clone()),
            },
            voltage: Complex64::from_polar(self.magnitudes[index], self.angles[index]),
        })
    }

    pub fn into_grounded(self) -> Result<VoltageSource> {
        if self.reference != VoltageReference::Earth {
            return Err(format_error(
                "floating source circuit requires typed reference-terminal transport",
            ));
        }
        Ok(self.into_source())
    }

    pub fn into_source(self) -> VoltageSource {
        let mut source = VoltageSource::new(
            self.name,
            self.bus,
            self.terminals.to_vec(),
            self.magnitudes.to_vec(),
            self.angles.to_vec(),
        );
        if let VoltageReference::Terminal(reference) = self.reference {
            source.reference_terminal = Some(reference);
        }
        source
    }
}

pub(super) struct IdealSourceCircuit {
    pub bus: DistBus,
    pub boundary: IdealVoltageBoundary,
    pub switch: DistSwitch,
    /// Finite native Z0 represented by Z0/3 from the local source star to earth.
    pub grounding_shunt: Option<DistShunt>,
}

impl IdealSourceCircuit {
    pub fn into_grounded(self) -> Result<SourceCircuit> {
        Ok(SourceCircuit {
            bus: self.bus,
            source: self.boundary.into_grounded()?,
            switch: self.switch,
        })
    }
}

impl InfeederInput {
    pub fn ideal_circuit(&self, bus: &DistBus, nominal_ll_volts: f64) -> Result<SourceCircuit> {
        self.ideal_boundary_circuit(bus, nominal_ll_volts)?
            .into_grounded()
    }

    pub fn ideal_boundary_circuit(
        &self,
        bus: &DistBus,
        nominal_ll_volts: f64,
    ) -> Result<IdealSourceCircuit> {
        if self.state != State::On
            || self.terminal.connection != Connection::L123
            || self.operating_series.iter().any(Option::is_some)
        {
            return Err(format_error(
                "ideal source requires static in-service L123 inputs",
            ));
        }
        if self.internal_impedance != InternalImpedance::Ideal {
            return Err(format_error(
                "finite source impedance requires explicit circuit assembly",
            ));
        }
        // xi=0 establishes only the positive-sequence ideal boundary. An
        // ungrounded source has no zero-sequence current path; its star is
        // an unknown, not an earth-referenced prescribed voltage.
        let (reference, grounding_admittance) = match self.grounding {
            SourceGrounding::Ungrounded => (VoltageReference::Terminal("star".into()), None),
            SourceGrounding::Solid(SourceZeroSequence::DirectOhms(z))
                if z.re == 0.0 && z.im == 0.0 =>
            {
                (VoltageReference::Earth, None)
            }
            SourceGrounding::Solid(SourceZeroSequence::DirectOhms(z)) => {
                // I_star=3 I0 and V0=-Z0 I0 for current delivered to the
                // network, hence the local grounding admittance is 3/Z0.
                // Scale the reciprocal to avoid squaring a large impedance.
                let scale = z.re.abs().max(z.im.abs());
                let unit = z / scale;
                let y = (3.0 / scale) * unit.conj() / unit.norm_sqr();
                if !y.re.is_finite() || !y.im.is_finite() || y == Complex64::default() {
                    return Err(format_error(
                        "source grounding admittance overflows or underflows",
                    ));
                }
                (VoltageReference::Terminal("star".into()), Some(y))
            }
            _ => {
                return Err(format_error(
                    "source requires direct solid-grounding R0/X0 or an ungrounded star",
                ));
            }
        };
        let InfeederSetpoint::Voltage {
            voltage, angle_rad, ..
        } = self.setpoint
        else {
            return Err(format_error(
                "source regulation mode requires separate mapping",
            ));
        };
        // With zero positive-sequence internal drop, both regulation
        // locations give the same prescribed line-line voltage. Floating
        // common-mode voltage is determined by the external circuit.
        let magnitude = resolve_voltage(voltage, nominal_ll_volts)? / 3.0_f64.sqrt();
        if magnitude <= 0.0 || !angle_rad.is_finite() {
            return Err(format_error("invalid source phase voltage"));
        }
        let phases = ["1", "2", "3"].map(str::to_owned).to_vec();
        if bus.id != self.terminal.node.to_string()
            || phases.iter().any(|p| !bus.terminals.contains(p))
        {
            return Err(format_error("source bus identity or phase map mismatch"));
        }
        let internal = format!("sincal:infeeder:{}", self.element);
        let boundary = IdealVoltageBoundary {
            name: self.element.to_string(),
            bus: internal.clone(),
            terminals: ["1", "2", "3"].map(str::to_owned),
            reference,
            magnitudes: [magnitude; 3],
            angles: [angle_rad, angle_rad - TAU / 3.0, angle_rad + TAU / 3.0],
        };
        let switch = DistSwitch::new(
            format!("sincal:terminal:{}", self.terminal.id),
            bus.id.clone(),
            internal.clone(),
            phases.clone(),
            phases.clone(),
            self.terminal.state == State::Off,
        );
        let mut local_bus = DistBus::new(internal, phases);
        if let VoltageReference::Terminal(terminal) = &boundary.reference {
            local_bus.terminals.push(terminal.clone());
        }
        let grounding_shunt = grounding_admittance.map(|y| {
            DistShunt::new(
                format!("sincal:infeeder:{}:zero-sequence", self.element),
                local_bus.id.clone(),
                vec!["star".into()],
                vec![vec![y.re]],
                vec![vec![y.im]],
            )
        });
        Ok(IdealSourceCircuit {
            bus: local_bus,
            boundary,
            switch,
            grounding_shunt,
        })
    }
}

impl super::schema::NativeDatabase {
    /// Direct native R0/X0 refers to the selected short-circuit data set.
    /// Only the current-data selection is implemented. Min/max selection must
    /// not reuse these fields. Siemens Input Data (2014), printed pp. 50–51.
    pub fn require_source_sequence_selection(&self, input: &InfeederInput) -> Result<()> {
        if matches!(
            input.grounding,
            SourceGrounding::Solid(
                SourceZeroSequence::MagnitudeRatio { .. } | SourceZeroSequence::SameAsPositive
            )
        ) || matches!(input.grounding, SourceGrounding::Solid(SourceZeroSequence::DirectOhms(z)) if z != Complex64::default())
        {
            if self.version.to_bits() != 11.5_f64.to_bits() {
                return Err(format_error(
                    "finite source zero sequence requires the verified schema-11.5 profile",
                ));
            }
            let mut statement = self
                .connection
                .prepare("SELECT Flag_ScType FROM CalcParameter WHERE Variant_ID=?1")
                .map_err(format_error)?;
            let mut rows = statement.query([self.variant]).map_err(format_error)?;
            let row = rows
                .next()
                .map_err(format_error)?
                .ok_or_else(|| format_error("missing source calculation settings"))?;
            if row.get::<_, i64>(0).map_err(format_error)? != 1
                || rows.next().map_err(format_error)?.is_some()
            {
                return Err(format_error(
                    "source zero sequence requires unambiguous current short-circuit data selection",
                ));
            }
        }
        Ok(())
    }
}
