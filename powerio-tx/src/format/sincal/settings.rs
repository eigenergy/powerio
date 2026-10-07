//! Active global controls must be checked alongside component references.
use super::{DatabaseSnapshot, NativeRow, Result, table};

pub(super) struct StaticProfiles {
    time: bool,
    operating: bool,
    increase: bool,
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

pub(super) fn validate(db: &DatabaseSnapshot, settings: &NativeRow) -> Result<StaticProfiles> {
    settings.inactive(&[
        "Flag_UseLA",
        "OpSer_ID",
        "IncrSer_ID",
        "Scenario_ID",
        "Flag_UseScenario",
    ])?;
    let profiles = StaticProfiles {
        time: settings.state("Flag_UseTimeSer")?,
        operating: settings.state("Flag_UseOpSer")?,
        increase: settings.state("Flag_UseIncSer")?,
    };
    // Flag_Unit controls interchange, not electrical units. A global enable
    // with no enabled group/transfer does not change the static equations.
    let interchange = settings.state("Flag_Unit")?;
    for group in table(db, "NetworkGroup", "Group_ID")?.values() {
        group.inactive(&["Flag_Temp"])?;
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
