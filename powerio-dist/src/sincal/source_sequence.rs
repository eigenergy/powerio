//! Resolve source zero sequence against selected short-circuit input, never
//! against the independently specified positive-sequence load-flow impedance.
use num_complex::Complex64;

use super::{
    format_error,
    infeeder::{InfeederInput, SourceGrounding, SourceZeroSequence},
    schema::NativeDatabase,
    semantics::require_input_categories,
    transformer::{integer, number},
};
use crate::Result;

impl NativeDatabase {
    pub fn resolve_source_sequence(&self, input: &mut InfeederInput) -> Result<()> {
        self.require_source_sequence_selection(input)?;
        let SourceGrounding::Solid(sequence) = input.grounding else {
            return Ok(());
        };
        if matches!(sequence, SourceZeroSequence::DirectOhms(_)) {
            return Ok(());
        }
        let mut stmt = self
            .connection
            .prepare(
                "SELECT e.Flag_Input AS ElementInput, i.* FROM Infeeder i JOIN Element e
             ON e.Element_ID=i.Element_ID AND e.Variant_ID=i.Variant_ID
             WHERE i.Element_ID=?1 AND i.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let z = stmt.query_row([input.element,self.variant], |row| {
            let resolve = || -> Result<Complex64> {
                require_input_categories(integer(row,"ElementInput")?, 1)?;
                if integer(row,"Flag_Typ")? != 1 {
                    return Err(format_error("source sequence ratio requires verified current R/X short-circuit input"));
                }
                let z1 = Complex64::new(number(row,"R")?,number(row,"X")?);
                if z1.re < 0.0 || z1 == Complex64::default() || !z1.re.hypot(z1.im).is_finite() {
                    return Err(format_error("invalid source short-circuit impedance"));
                }
                let z0 = match sequence {
                    SourceZeroSequence::MagnitudeRatio {z0_over_z1,r0_over_x0} => {
                        let magnitude = z1.re.hypot(z1.im) * z0_over_z1;
                        let denominator = r0_over_x0.hypot(1.0);
                        Complex64::new(magnitude*(r0_over_x0/denominator),magnitude/denominator)
                    }
                    SourceZeroSequence::SameAsPositive => z1,
                    SourceZeroSequence::DirectOhms(_) => unreachable!(),
                };
                if !z0.re.is_finite() || !z0.im.is_finite()
                    || (matches!(sequence,SourceZeroSequence::MagnitudeRatio {z0_over_z1,..} if z0_over_z1>0.0) && z0 == Complex64::default()) {
                    return Err(format_error("source zero-sequence impedance overflows or underflows"));
                }
                Ok(z0)
            };
            Ok(resolve())
        }).map_err(format_error)??;
        input.grounding = SourceGrounding::Solid(SourceZeroSequence::DirectOhms(z));
        Ok(())
    }
}
