//! Nominal full-winding transformer primitives, including all three sequences.
//!
//! A six-terminal coupled shunt on an auxiliary bus represents the finite
//! primitive. Typed switches connect those coordinates to the native ports.
//! Native star grounding is represented by the zero-sequence circuit, never
//! by grounding a phase or an unrelated explicit neutral on a native bus.

use std::{collections::BTreeMap, f64::consts::PI};

use num_complex::Complex64;

use super::{
    format_error,
    schema::NativeDatabase,
    semantics::State,
    transformer::{integer, number, reference},
    transformer_impedance::{NominalTransformerInput, ZeroSequenceInput},
};
use crate::{DistBus, DistShunt, DistSwitch, Result};

type TwoPort = [[Complex64; 2]; 2];
const ZERO: Complex64 = Complex64::new(0.0, 0.0);

pub(super) struct TransformerCircuit {
    pub auxiliary_bus: DistBus,
    pub shunt: DistShunt,
    pub terminal_switches: Vec<DistSwitch>,
}

impl NativeDatabase {
    pub fn transformer_circuit(
        &self,
        element: i64,
        buses: &BTreeMap<i64, DistBus>,
    ) -> Result<TransformerCircuit> {
        self.require_input_zero_sequence()?;
        self.require_nominal_transformer_mode(element)?;
        let input = self.transformer_nominal(element)?;
        let connection = &input.connection;
        let primitive = phase_admittance(&input)?;
        let bus_id = format!("sincal:transformer:{element}");
        let coordinates: Vec<String> = ["p1", "p2", "p3", "s1", "s2", "s3"]
            .map(str::to_owned)
            .to_vec();
        let auxiliary_bus = DistBus::new(bus_id.clone(), coordinates.clone());
        let mut switches = Vec::new();
        for (side, port) in connection.ports.iter().enumerate() {
            let bus = buses
                .get(&port.node)
                .ok_or_else(|| format_error("transformer bus missing"))?;
            let phases = ["1", "2", "3"].map(str::to_owned).to_vec();
            if bus.id != port.node.to_string() || phases.iter().any(|p| !bus.terminals.contains(p))
            {
                return Err(format_error(
                    "transformer bus identity or phase map mismatch",
                ));
            }
            switches.push(DistSwitch::new(
                format!("sincal:terminal:{}", port.id),
                bus.id.clone(),
                bus_id.clone(),
                phases,
                coordinates[3 * side..3 * side + 3].to_vec(),
                port.state == State::Off,
            ));
        }
        let mut shunt = DistShunt::new(
            element.to_string(),
            bus_id,
            coordinates,
            primitive
                .iter()
                .map(|row| row.iter().map(|v| v.re).collect())
                .collect(),
            primitive
                .iter()
                .map(|row| row.iter().map(|v| v.im).collect())
                .collect(),
        );
        shunt.extras.insert(
            "sincal_transformer".into(),
            serde_json::json!({
                "element": element, "clock":connection.vector_group.clock,
                "terminal_ids":connection.ports.iter().map(|p| p.id).collect::<Vec<_>>()
            }),
        );
        Ok(TransformerCircuit {
            auxiliary_bus,
            shunt,
            terminal_switches: switches,
        })
    }

    fn require_nominal_transformer_mode(&self, element: i64) -> Result<()> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM TwoWindingTransformer WHERE Element_ID=?1 AND Variant_ID=?2")
            .map_err(format_error)?;
        statement
            .query_row([element, self.variant], |row| {
                let check = || -> Result<()> {
                    if integer(row, "Flag_Lf")? != 1
                        || integer(row, "Flag_Macro")? != 0
                        || integer(row, "Flag_Boost")? != 0
                        || number(row, "C01")? != 0.0
                        || number(row, "C02")? != 0.0
                    {
                        return Err(format_error(
                            "transformer model/boost/capacitance requires separate mapping",
                        ));
                    }
                    for field in [
                        "ElemLoading_ID",
                        "Ctrl_OpSer_ID",
                        "Ctrl_OpPnt_ID",
                        "CompImp_ID",
                        "CtrlRange_ID",
                    ] {
                        if reference(row, field)?.is_some() {
                            return Err(format_error(format!(
                                "transformer requires resolution of {field}"
                            )));
                        }
                    }
                    Ok(())
                };
                Ok(check())
            })
            .map_err(format_error)??;
        Ok(())
    }
}

fn checked(value: Complex64) -> Result<Complex64> {
    if value.re.is_finite() && value.im.is_finite() {
        Ok(value)
    } else {
        Err(format_error("transformer primitive overflows"))
    }
}

fn reciprocal(value: Complex64) -> Result<Complex64> {
    if value == ZERO {
        return Err(format_error(
            "zero transformer impedance needs an ideal constraint",
        ));
    }
    let result = checked(Complex64::new(1.0, 0.0) / value)?;
    if result == ZERO {
        return Err(format_error("transformer admittance underflows"));
    }
    Ok(result)
}

fn refer_primary(mut port: TwoPort, ratio: Complex64) -> Result<TwoPort> {
    port[0][0] = checked(port[0][0] / ratio / ratio.conj())?;
    port[0][1] = checked(port[0][1] / ratio.conj())?;
    port[1][0] = checked(port[1][0] / ratio)?;
    Ok(port)
}

fn zero_port(input: &NominalTransformerInput, ratio: f64) -> Result<TwoPort> {
    let mut port = [[ZERO; 2]; 2];
    match input.zero_sequence {
        ZeroSequenceInput::NoGroundPath => {}
        ZeroSequenceInput::GroundedSide {
            side,
            impedance_ohm,
        } => {
            port[side][side] = reciprocal(impedance_ohm)?;
        }
        ZeroSequenceInput::BothGrounded {
            open_primary_ohm,
            open_secondary_ohm,
            short_primary_ohm,
        } => {
            // Input Data (2014), pp. 185–188. Refer measured primary ohms
            // to the secondary before using Zabl=Z1+Z3, Zbal=Z2+Z3 and
            // Zabk=Z1+Z2*Z3/(Z2+Z3). Hence Z3²=(Zabl-Zabk)*Zbal.
            let za = checked(open_primary_ohm / ratio / ratio)?;
            let zb = open_secondary_ohm;
            let zsc = checked(short_primary_ohm / ratio / ratio)?;
            let zm = checked(checked((za - zsc) * zb)?.sqrt())?;
            // Winding polarity supplies the transfer sign. The remaining
            // ambiguity is resolved only for an inductive passive central
            // T branch; do not infer a capacitive core from these inputs.
            if zm.im < 0.0 {
                return Err(format_error(
                    "transformer measurements require an unverified central T branch",
                ));
            }
            // Passive measured impedance must have a positive-semidefinite
            // resistive part. Normalize before checking its determinant.
            let scale = za.re.max(zb.re).max(zm.re.abs());
            if scale > 0.0
                && (za.re / scale) * (zb.re / scale) - (zm.re / scale).powi(2)
                    < -64.0 * f64::EPSILON
            {
                return Err(format_error(
                    "inconsistent transformer open/short measurements",
                ));
            }
            // The determinant is Zsc*Zbal. Use the measured short-circuit
            // value directly instead of subtracting two nearly equal
            // products when the magnetizing impedance is large.
            let inverse = reciprocal(checked(zsc * zb)?)?;
            port = [
                [checked(zb * inverse)?, checked(-zm * inverse)?],
                [checked(-zm * inverse)?, checked(za * inverse)?],
            ];
            let polarity = if input.connection.vector_group.clock == 6 {
                -1.0
            } else {
                1.0
            };
            port = refer_primary(port, Complex64::new(polarity * ratio, 0.0))?;
        }
    }
    Ok(port)
}

pub(super) fn phase_admittance(input: &NominalTransformerInput) -> Result<[[Complex64; 6]; 6]> {
    let connection = &input.connection;
    if connection.state != State::On
        || connection.coils.len() != 3
        || connection.neutral_points.iter().any(Option::is_some)
        || connection
            .tap
            .positions
            .iter()
            .any(|p| p.is_none_or(|p| p.to_bits() != connection.tap.midpoint.to_bits()))
    {
        return Err(format_error(
            "transformer primitive requires in-service full windings, nominal taps and resolved solid/no grounding",
        ));
    }
    // Input Data (April 2014), printed p. 181: the additional rotation
    // adds to the vector-group rotation. The opposite rotating sequence
    // uses the conjugate ratio. A grounded zero-sequence circuit needs
    // separate evidence before extending this profile to arbitrary rotation.
    if connection.additional_rotation_rad != 0.0
        && !matches!(input.zero_sequence, ZeroSequenceInput::NoGroundPath)
    {
        return Err(format_error(
            "additional transformer rotation with grounding requires separate mapping",
        ));
    }
    let ratio = connection.rated_ll_volts[0] / connection.rated_ll_volts[1];
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err(format_error("invalid nominal transformer ratio"));
    }
    let series = reciprocal(input.series_secondary_ohm)?;
    let diagonal = checked(series + input.no_load_secondary_siemens / 2.0)?;
    let nominal = [[diagonal, -series], [-series, diagonal]];
    let rotation = Complex64::from_polar(
        ratio,
        f64::from(connection.vector_group.clock) * PI / 6.0 + connection.additional_rotation_rad,
    );
    let positive = refer_primary(nominal, rotation)?;
    let negative = refer_primary(nominal, rotation.conj())?;
    let zero = zero_port(input, ratio)?;
    let mut result = [[ZERO; 6]; 6];
    // F diag(Y0,Y1,Y2) F^-1 for each port block. Positive sequence uses
    // [1,a²,a], negative uses [1,a,a²]; arbitrary unbalanced voltages survive.
    let phases = [
        Complex64::new(1.0, 0.0),
        Complex64::new(-0.5, -3.0_f64.sqrt() / 2.0),
        Complex64::new(-0.5, 3.0_f64.sqrt() / 2.0),
    ];
    for from in 0..2 {
        for to in 0..2 {
            for row in 0..3 {
                for column in 0..3 {
                    let factor = phases[row] * phases[column].conj();
                    result[3 * from + row][3 * to + column] = checked(
                        (zero[from][to]
                            + positive[from][to] * factor
                            + negative[from][to] * factor.conj())
                            / 3.0,
                    )?;
                }
            }
        }
    }
    Ok(result)
}
