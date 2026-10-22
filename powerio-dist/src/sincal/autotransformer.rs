//! Same-voltage Y0 regulators at the exact neutral tap. Input Data (April
//! 2014), pp.181–182: Zact=Zchar*((1-uact)/(1-uchar))^2. At uact=1 this
//! is an ideal conductive connection, not a tiny-impedance transformer.
use std::collections::BTreeMap;

use super::{
    format_error,
    schema::NativeDatabase,
    semantics::{State, require_input_categories},
    transformer::{TransformerConnectionInput, WindingKind, integer, number},
};
use crate::{DistBus, DistSwitch, Result};

impl NativeDatabase {
    #[allow(clippy::float_cmp)] // Exact neutral tap/equal ratings select an ideal constraint.
    pub fn ideal_autotransformer_switch(
        &self,
        element: i64,
        buses: &BTreeMap<i64, DistBus>,
    ) -> Result<Option<DistSwitch>> {
        let connection = self.transformer_connection(element)?;
        let group = connection.vector_group;
        if !group.autotransformer
            || group.primary
                != (WindingKind::Wye {
                    grounding_enabled: false,
                })
            || connection.rated_ll_volts[0] != connection.rated_ll_volts[1]
        {
            return Ok(None);
        }
        self.require_nominal_transformer_mode(element)?;
        let categories: i64 = self
            .connection
            .query_row(
                "SELECT Flag_Input FROM Element WHERE Element_ID=?1 AND Variant_ID=?2",
                [element, self.variant],
                |r| r.get(0),
            )
            .map_err(format_error)?;
        require_input_categories(categories, 6)?;
        if connection.additional_rotation_rad != 0.0
            || connection.neutral_points.iter().any(Option::is_some)
            || connection
                .tap
                .positions
                .iter()
                .flatten()
                .any(|p| *p != connection.tap.midpoint)
            || connection.tap.boost_angle_rad != 0.0
            || connection.tap.rotation_per_step_rad != 0.0
        {
            return Err(format_error(
                "Y0 ideal connection requires neutral fixed taps and no rotation/grounding",
            ));
        }
        self.require_ideal_characteristic(element, &connection)?;
        let phases: Vec<_> = connection
            .coils
            .iter()
            .map(|c| (c.winding + 1).to_string())
            .collect();
        for port in &connection.ports {
            let bus = buses
                .get(&port.node)
                .ok_or_else(|| format_error("autotransformer bus missing"))?;
            if bus.id != port.node.to_string() || phases.iter().any(|p| !bus.terminals.contains(p))
            {
                return Err(format_error("autotransformer bus or phase mismatch"));
            }
        }
        let terminal_closed: Vec<_> = connection
            .ports
            .iter()
            .map(|p| p.state == State::On)
            .collect();
        let in_service = connection.state == State::On;
        let mut switch = DistSwitch::new(
            element.to_string(),
            connection.ports[0].node.to_string(),
            connection.ports[1].node.to_string(),
            phases.clone(),
            phases,
            !in_service || terminal_closed.contains(&false),
        );
        switch.extras.insert("sincal_autotransformer".into(),serde_json::json!({"group":"Y0","profile":"neutral_tap_ideal_connection","element":element,"terminal_ids":connection.ports.iter().map(|p|p.id).collect::<Vec<_>>(),"terminal_closed":terminal_closed,"in_service":in_service}));
        Ok(Some(switch))
    }
    fn require_ideal_characteristic(
        &self,
        element: i64,
        connection: &TransformerConnectionInput,
    ) -> Result<()> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM TwoWindingTransformer WHERE Element_ID=?1 AND Variant_ID=?2")
            .map_err(format_error)?;
        statement
            .query_row([element, self.variant], |row| {
                let check = || -> Result<()> {
                    if self.legacy_transformer_number(row, "Vfe")? != 0.0
                        || self.legacy_transformer_number(row, "i0")? != 0.0
                    {
                        return Err(format_error(
                            "ideal autotransformer requires zero excitation",
                        ));
                    }
                    let (low, high) = (number(row, "rohl")?, number(row, "rohu")?);
                    let mid = connection.tap.midpoint;
                    let step = connection.tap.step_fraction;
                    let span = step * (high - low);
                    if !(low < mid
                        && mid < high
                        && step > 0.0
                        && 1.0 + span / 2.0 > 1.0
                        && (1.0 + step * (low - mid)) > 0.0
                        && span.is_finite())
                    {
                        return Err(format_error(
                            "undefined characteristic autotransformer ratio",
                        ));
                    }
                    let (uk, ur) = (number(row, "uk")?, number(row, "ur")?);
                    if uk <= 0.0
                        || ur < 0.0
                        || ur > uk
                        || integer(row, "Flag_Z0_Input")? != 3
                        || number(row, "R0_R1")? < 0.0
                        || number(row, "X0_X1")? < 0.0
                    {
                        return Err(format_error(
                            "invalid or unresolved characteristic autotransformer impedance",
                        ));
                    }
                    Ok(())
                };
                Ok(check())
            })
            .map_err(format_error)??;
        Ok(())
    }
}
