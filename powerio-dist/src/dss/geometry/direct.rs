//! Live direct Line.spacing inputs; the Line owns its calculation-frequency cache.
use super::{EarthModel, Geometry, cache::Context, wire};
use crate::dss::linecode::LineMatrices;

#[derive(Clone, Debug)]
pub(in crate::dss) struct Binding {
    pub object: usize,
    pub conductors: Vec<Option<usize>>,
    pub cable: bool,
}

impl Binding {
    pub fn calculate(
        &self,
        context: &mut Context<'_>,
        frequency: f64,
        earth: EarthModel,
        rho: Option<f64>,
    ) -> Result<(LineMatrices, f64, f64), String> {
        let spacing = context.spacing(self.object)?;
        if spacing.x.len() != self.conductors.len() {
            return Err("live LineSpacing size differs from its assigned conductor slots".into());
        }
        let mut normamps = 0.0;
        let mut emergamps = 0.0;
        let meters = wire::meters(spacing.units);
        let wires = self
            .conductors
            .iter()
            .enumerate()
            .map(|(i, object)| {
                let state = context
                    .conductor(object.ok_or("direct LineSpacing has an unassigned conductor")?)?;
                if i == 0 {
                    normamps = state.normamps;
                    emergamps = state.emergamps;
                }
                Ok(state.at(spacing.x[i] * meters, spacing.height[i] * meters))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut input = Geometry {
            wires,
            phases: spacing.phases,
            reduce: spacing.phases < spacing.x.len(),
            reduction_order: None,
            earth_model: context.active_earth.clone()?,
            rho: 100.0,
        };
        // LoadSpacingAndWires calculates immediately using the PREVIOUS active
        // earth model. FMakeZFromSpacing selects the Line earth model afterwards.
        let mut result = input.calculate(frequency)?;
        context.active_earth = Ok(earth);
        if let Some(rho) = rho
            && rho.to_bits() != 100.0_f64.to_bits()
        {
            input.rho = rho;
            input.earth_model = earth;
            result = input.calculate(frequency)?;
        }
        Ok((result, normamps, emergamps))
    }
}
