//! Active global controls must be checked alongside component references.
use super::{DatabaseSnapshot, NativeRow, Result, table};

pub(super) struct StaticProfiles {
    time: bool,
    operating: bool,
    increase: bool,
    pub hours: Option<f64>,
}

impl StaticProfiles {
    pub fn check(&self, row: &NativeRow, has_growth_reference: bool) -> Result<()> {
        // Disabled profiles do not alter the static setpoint. Do not load
        // their values, infer a timestamp, or import stored result tables.
        for (active, fields) in [
            (self.time, &["DayOpSer_ID", "YearOpSer_ID"][..]),
            (self.operating, &["WeekOpSer_ID"][..]),
            (self.increase, &["IncrSer_ID"][..]),
        ] {
            if fields == ["IncrSer_ID"] && !has_growth_reference {
                continue;
            }
            if active {
                row.inactive(fields)?;
            } else {
                for field in fields {
                    row.reference(field)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn validate(
    db: &DatabaseSnapshot,
    settings: &NativeRow,
    hours: Option<f64>,
) -> Result<StaticProfiles> {
    settings.inactive_newer(&[
        "Flag_UseLA",
        "OpSer_ID",
        "IncrSer_ID",
        "Scenario_ID",
        "Flag_UseScenario",
    ])?;
    if settings.legacy() && settings.number("Temp_Cond")?.to_bits() != 20.0_f64.to_bits() {
        return Err(settings.bad(
            "Temp_Cond",
            "calculation temperature override requires mapping",
        ));
    }
    let profiles = StaticProfiles {
        time: if settings.legacy() {
            true
        } else {
            settings.state("Flag_UseTimeSer")?
        },
        operating: if settings.legacy() {
            true
        } else {
            settings.state("Flag_UseOpSer")?
        },
        increase: if settings.legacy() {
            true
        } else {
            settings.state("Flag_UseIncSer")?
        },
        hours,
    };
    // Flag_Unit controls interchange, not electrical units. A global enable
    // with no enabled group/transfer does not change the static equations.
    let interchange = settings.state("Flag_Unit")?;
    for group in table(db, "NetworkGroup", "Group_ID")?.values() {
        group.inactive_newer(&["Flag_Temp"])?;
        if interchange {
            group.inactive(&["Flag_IC"])?;
        }
    }
    if interchange {
        for transfer in table(db, "NetworkGroupTrans", "GroupTrans_ID")?.values() {
            transfer.inactive(&["Flag_State"])?;
        }
    }
    Ok(profiles)
}
