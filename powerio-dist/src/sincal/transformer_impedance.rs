//! Nominal transformer sequence inputs, with their measurement sides explicit.
//!
//! Siemens Input Data (April 2014), printed pp. 180 and 185–188. These
//! parameters do not include tap, rotation or neutral-grounding corrections.
//! A complete conductor-domain primitive must combine those independently.

use num_complex::Complex64;
use rusqlite::Row;

use super::{
    format_error,
    schema::NativeDatabase,
    semantics::require_input_categories,
    transformer::{TransformerConnectionInput, WindingKind, integer, number},
};
use crate::Result;

#[derive(Debug)]
pub(super) enum ZeroSequenceInput {
    /// Neither winding has native N grounding enabled. No external
    /// zero-sequence ground path; this is not a zero-ohm impedance.
    NoGroundPath,
    GroundedSide {
        side: usize,
        impedance_ohm: Complex64,
    },
    /// Measured open/short impedances retain their original reference sides.
    /// They must not be combined until referred to a common voltage base.
    BothGrounded {
        open_primary_ohm: Complex64,
        open_secondary_ohm: Complex64,
        short_primary_ohm: Complex64,
    },
}

pub(super) struct NominalTransformerInput {
    pub connection: TransformerConnectionInput,
    /// Positive sequence, referred to rated secondary line-line voltage.
    pub series_secondary_ohm: Complex64,
    /// Total nominal no-load admittance, referred to the secondary. The
    /// native equivalent circuit divides it equally between its two ends.
    pub no_load_secondary_siemens: Complex64,
    pub zero_sequence: ZeroSequenceInput,
}

impl NativeDatabase {
    pub fn transformer_nominal(&self, element: i64) -> Result<NominalTransformerInput> {
        let connection = self.transformer_connection(element)?;
        let mut statement = self.connection.prepare(
            "SELECT e.Flag_Input AS ElementInput, t.* FROM TwoWindingTransformer t JOIN Element e
             ON e.Element_ID=t.Element_ID AND e.Variant_ID=t.Variant_ID
             WHERE t.Element_ID=?1 AND t.Variant_ID=?2").map_err(format_error)?;
        let input = statement
            .query_row([element, self.variant], |row| Ok(decode(row, connection)))
            .map_err(format_error)??;
        Ok(input)
    }
}

fn decode(
    row: &Row<'_>,
    connection: TransformerConnectionInput,
) -> Result<NominalTransformerInput> {
    let volts_squared = connection.rated_ll_volts[1].powi(2);
    let impedance_base = finite(volts_squared / connection.rated_va, "impedance base")?;
    if impedance_base <= 0.0 {
        return Err(format_error("transformer impedance base underflows"));
    }
    let magnitude = nonnegative(row, "uk")? / 100.0;
    let resistance = nonnegative(row, "ur")? / 100.0;
    let series_secondary_ohm = checked(Complex64::new(
        impedance_base * resistance,
        impedance_base * quadrature(magnitude, resistance)?,
    ))?;
    let core_watts = finite(nonnegative(row, "Vfe")? * 1000.0, "core loss")?;
    let no_load_va = finite(
        nonnegative(row, "i0")? / 100.0 * connection.rated_va,
        "no-load VA",
    )?;
    let no_load_secondary_siemens = checked(Complex64::new(
        core_watts / volts_squared,
        -quadrature(no_load_va, core_watts)? / volts_squared,
    ))?;
    let zero_sequence = zero_sequence(row, &connection, series_secondary_ohm)?;
    Ok(NominalTransformerInput {
        connection,
        series_secondary_ohm,
        no_load_secondary_siemens,
        zero_sequence,
    })
}

fn zero_sequence(
    row: &Row<'_>,
    connection: &TransformerConnectionInput,
    secondary: Complex64,
) -> Result<ZeroSequenceInput> {
    let grounded = [
        connection.vector_group.primary,
        connection.vector_group.secondary,
    ]
    .map(|kind| {
        matches!(
            kind,
            WindingKind::Wye {
                grounding_enabled: true
            }
        )
    });
    if grounded == [false, false] {
        return Ok(ZeroSequenceInput::NoGroundPath);
    }
    require_input_categories(integer(row, "ElementInput")?, 6)?;
    let mode = integer(row, "Flag_Z0_Input")?;
    if grounded == [true, true] {
        if mode != 4 {
            return Err(format_error(
                "both-grounded transformer requires open/short measurements",
            ));
        }
        return Ok(ZeroSequenceInput::BothGrounded {
            open_primary_ohm: measured(row, "ZABNL", "RX_ZABNL")?,
            open_secondary_ohm: measured(row, "ZBANL", "RX_ZBANL")?,
            short_primary_ohm: measured(row, "ZABSC", "RX_ZABSC")?,
        });
    }
    let side = usize::from(!grounded[0]);
    // Direct ohmic measurements already use the grounded side's voltage
    // basis; do not rescale them through an unused positive-sequence ratio.
    if mode == 2 {
        return Ok(ZeroSequenceInput::GroundedSide {
            side,
            impedance_ohm: Complex64::new(nonnegative(row, "R0")?, nonnegative(row, "X0")?),
        });
    }
    let ratio = connection.rated_ll_volts[side] / connection.rated_ll_volts[1];
    let positive = checked(secondary * ratio * ratio)?;
    let impedance_ohm = match mode {
        1 => from_magnitude_rx(
            finite(
                positive.re.hypot(positive.im) * nonnegative(row, "Z0_Z1")?,
                "zero-sequence magnitude",
            )?,
            nonnegative(row, "R0_X0")?,
        )?,
        3 => checked(Complex64::new(
            positive.re * nonnegative(row, "R0_R1")?,
            positive.im * nonnegative(row, "X0_X1")?,
        ))?,
        _ => {
            return Err(format_error(
                "unsupported grounded-side zero-sequence input mode",
            ));
        }
    };
    Ok(ZeroSequenceInput::GroundedSide {
        side,
        impedance_ohm,
    })
}

fn measured(row: &Row<'_>, magnitude: &str, rx: &str) -> Result<Complex64> {
    from_magnitude_rx(nonnegative(row, magnitude)?, nonnegative(row, rx)?)
}

fn from_magnitude_rx(magnitude: f64, rx: f64) -> Result<Complex64> {
    let denominator = rx.hypot(1.0);
    checked(Complex64::new(
        magnitude * (rx / denominator),
        magnitude / denominator,
    ))
}

fn quadrature(magnitude: f64, real: f64) -> Result<f64> {
    if real > magnitude {
        // Independently scaled nameplate quantities can straddle an exact
        // P=|S| (or R=|Z|) boundary by a few floating-point rounding errors.
        // There is no absolute tolerance: a positive value against zero,
        // or a material excess at any scale, still rejects the measurements.
        if magnitude > 0.0 && real - magnitude <= 8.0 * f64::EPSILON * real {
            return Ok(0.0);
        }
        return Err(format_error(
            "transformer real component exceeds stated magnitude",
        ));
    }
    if magnitude == 0.0 {
        return Ok(0.0);
    }
    let ratio = real / magnitude;
    finite(
        magnitude * ((1.0 - ratio) * (1.0 + ratio)).sqrt(),
        "reactive component",
    )
}

fn nonnegative(row: &Row<'_>, field: &str) -> Result<f64> {
    let value = number(row, field)?;
    if value < 0.0 {
        return Err(format_error(format!("negative transformer {field}")));
    }
    Ok(value)
}

fn finite(value: f64, field: &str) -> Result<f64> {
    if !value.is_finite() {
        return Err(format_error(format!("nonfinite transformer {field}")));
    }
    Ok(value)
}

fn checked(value: Complex64) -> Result<Complex64> {
    finite(value.re, "real impedance/admittance")?;
    finite(value.im, "imaginary impedance/admittance")?;
    Ok(value)
}
