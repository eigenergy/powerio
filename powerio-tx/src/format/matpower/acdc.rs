//! Strict 8/9-column DC bus, 9-column branch and 34-column station profile.
use super::{locate::Assignment, matlab};
use crate::acdc::{
    AcConverterControl, AcCurrentLosses, AcDcConverterStation, AcDcLimits, BalancedAcDcEquipment,
    ConverterSeries, ConverterSettings, ConverterTechnology, DcConverterControl,
    DcEquivalentBranch, DcEquivalentBus, invalid,
};
use crate::collect::Diagnostics;
use crate::{BalancedNetwork, BusId, Result};
use powerio_core::ComponentId;
use std::fmt::Write as _;

const BUS: &str = "busdc_i grid Pdc Vdc basekVdc Vdcmax Vdcmin Cdc";
const BRANCH: &str = "fbusdc tbusdc r l c rateA rateB rateC status";
const CONVERTER: &str = "busdc_i busac_i type_dc type_ac P_g Q_g islcc Vtar rtf xtf transformer tm bf filter rc xc reactor basekVac Vmmax Vmmin Imax status LossA LossB LossCrec LossCinv droop Pdcset Vdcset dVdcset Pacmax Pacmin Qacmax Qacmin";

fn id(kind: &str, value: impl ToString + Copy) -> ComponentId {
    ComponentId::new(kind, value.to_string()).expect("bounded numeric source identity")
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn integer(v: f64) -> Result<u32> {
    if !v.is_finite() || v < 0.0 || v > f64::from(u32::MAX) || v.fract() != 0.0 {
        return Err(invalid(format!(
            "expected a nonnegative 32-bit integer, got {v}"
        )));
    }
    Ok(v as u32)
}
fn flag(v: f64) -> Result<bool> {
    match integer(v)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid("flag must be 0 or 1")),
    }
}
fn limits(min: f64, max: f64) -> AcDcLimits {
    AcDcLimits { min, max }
}
fn rows(
    source: &str,
    assignments: &[Assignment<'_>],
    names: &[&'static str],
    columns: &str,
    optional_area: bool,
    warnings: &mut Diagnostics,
) -> Result<Option<Vec<Vec<f64>>>> {
    let found: Vec<_> = assignments
        .iter()
        .filter(|a| names.contains(&a.field))
        .collect();
    if found.len() > 1 {
        return Err(invalid(format!(
            "duplicate or conflicting aliases for {}",
            names[0]
        )));
    }
    let Some(a) = found.first() else {
        return Ok(None);
    };
    // Headers describe a fixed electrical layout; they cannot redefine it.
    let header = source[..a.start]
        .lines()
        .rev()
        .take_while(|l| l.trim().is_empty() || l.trim().starts_with('%'))
        .find_map(|l| {
            l.split_once("%column_names%")
                .map(|(_, h)| h.split_whitespace().collect::<Vec<_>>())
        });
    let expected: Vec<_> = columns.split_whitespace().collect();
    let mut width = expected.len();
    if let Some(h) = header {
        if optional_area && h.last() == Some(&"area") && h[..h.len() - 1] == expected {
            width += 1;
        } else if h != expected {
            return Err(invalid(format!("unsupported {} column header", a.field)));
        }
    }
    let mut result = Vec::new();
    matlab::for_each_matrix_row(a.text, a.start, names[0], warnings, |row, _| {
        if row.len() != width || row.iter().any(|v| !v.is_finite()) {
            return Err(invalid(format!(
                "{} requires {width} finite columns (area requires a header), got {}",
                a.field,
                row.len()
            )));
        }
        result.push(row.to_vec());
        Ok(())
    })?;
    Ok(Some(result))
}

pub(super) fn read(
    source: &str,
    assignments: &[Assignment<'_>],
    net: &mut BalancedNetwork,
    warnings: &mut Diagnostics,
) -> Result<()> {
    for a in assignments {
        if a.field.ends_with("_ne") || matches!(a.field, "gendc" | "pfc" | "dcr" | "Pacset") {
            return Err(invalid(format!(
                "unsupported electrical extension {}",
                a.field
            )));
        }
    }
    let buses = rows(
        source,
        assignments,
        &["busdc", "dcbus"],
        BUS,
        true,
        warnings,
    )?;
    let branches = rows(
        source,
        assignments,
        &["branchdc", "dcbranch"],
        BRANCH,
        false,
        warnings,
    )?;
    let converters = rows(
        source,
        assignments,
        &["convdc", "dcconv"],
        CONVERTER,
        false,
        warnings,
    )?;
    let poles: Vec<_> = assignments.iter().filter(|a| a.field == "dcpol").collect();
    if poles.len() > 1 {
        return Err(invalid("duplicate dcpol"));
    }
    if buses.is_none() && branches.is_none() && converters.is_none() && poles.is_empty() {
        return Ok(());
    }
    let pole_count = poles
        .first()
        .map(|a| matlab::scalar_from_assignment(a.text, "dcpol"))
        .transpose()?
        .flatten();
    let nonempty = [&buses, &branches, &converters]
        .iter()
        .any(|t| t.as_ref().is_some_and(|r| !r.is_empty()));
    if !nonempty && pole_count.is_none() {
        return Ok(());
    }
    let pole_count = match pole_count {
        Some(1.0) => 1,
        Some(2.0) => 2,
        _ => return Err(invalid("matpower-acdc requires explicit dcpol = 1 or 2")),
    };
    if nonempty && (buses.is_none() || converters.is_none() || branches.is_none()) {
        return Err(invalid(
            "nonempty AC/DC profile requires busdc, convdc and branchdc (possibly empty)",
        ));
    }
    let mut equipment = BalancedAcDcEquipment {
        base_mva: net.base_mva(),
        pole_count,
        buses: Vec::new(),
        branches: Vec::new(),
        stations: Vec::new(),
    };
    for r in buses.unwrap_or_default() {
        let external_id = integer(r[0])?;
        equipment.buses.push(DcEquivalentBus {
            component: id("dc_equivalent_bus", external_id),
            external_id,
            source_grid: integer(r[1])?,
            withdrawal_mw: r[2],
            initial_voltage_pu: r[3],
            base_kv: r[4],
            voltage_limits_pu: limits(r[6], r[5]),
            capacitance_pu: r[7],
            area: r.get(8).copied().map(integer).transpose()?,
        });
    }
    for (i, r) in branches.unwrap_or_default().iter().enumerate() {
        equipment.branches.push(DcEquivalentBranch {
            component: id("dc_equivalent_branch", i + 1),
            from: id("dc_equivalent_bus", integer(r[0])?),
            to: id("dc_equivalent_bus", integer(r[1])?),
            resistance_pu: r[2],
            inductance_pu_per_second: r[3],
            capacitance_pu_seconds: r[4],
            ratings_mw: [r[5], r[6], r[7]],
            in_service: flag(r[8])?,
        });
    }
    for (i, r) in converters.unwrap_or_default().iter().enumerate() {
        equipment.stations.push(converter(r, i)?);
    }
    net.set_ac_dc_equivalent(Some(equipment))?;
    Ok(())
}

// Bases are declaration identities, not computed approximate quantities.
#[allow(clippy::float_cmp)]
pub(crate) fn write(net: &BalancedNetwork) -> Result<super::super::TextEmission> {
    let mut result = super::writer::write_matpower_conversion(net);
    let Some(e) = net.ac_dc_equivalent() else {
        return Ok(result);
    };
    e.validate(net)?;
    writeln!(result.text, "\nmpc.dcpol = {};", e.pole_count).unwrap();
    if e.base_mva != net.base_mva() {
        return Err(invalid(
            "fresh MATPOWER AC/DC output requires equal source and network MVA bases",
        ));
    }
    let area = e.buses.iter().any(|b| b.area.is_some());
    if area && e.buses.iter().any(|b| b.area.is_none()) {
        return Err(invalid(
            "DC area column cannot represent a mix of absent and present areas",
        ));
    }
    writeln!(
        result.text,
        "%column_names% {BUS}{}\nmpc.busdc = [",
        if area { " area" } else { "" }
    )
    .unwrap();
    for b in &e.buses {
        write!(
            result.text,
            "{} {} {} {} {} {} {} {}",
            b.external_id,
            b.source_grid,
            b.withdrawal_mw,
            b.initial_voltage_pu,
            b.base_kv,
            b.voltage_limits_pu.max,
            b.voltage_limits_pu.min,
            b.capacitance_pu
        )
        .unwrap();
        if let Some(a) = b.area {
            write!(result.text, " {a}").unwrap();
        }
        result.text.push_str(";\n");
    }
    writeln!(result.text, "];\n%column_names% {BRANCH}\nmpc.branchdc = [").unwrap();
    let bus_id = |id: &ComponentId| {
        e.buses
            .iter()
            .find(|b| &b.component == id)
            .expect("validated reference")
            .external_id
    };
    for b in &e.branches {
        writeln!(
            result.text,
            "{} {} {} {} {} {} {} {} {};",
            bus_id(&b.from),
            bus_id(&b.to),
            b.resistance_pu,
            b.inductance_pu_per_second,
            b.capacitance_pu_seconds,
            b.ratings_mw[0],
            b.ratings_mw[1],
            b.ratings_mw[2],
            u8::from(b.in_service)
        )
        .unwrap();
    }
    writeln!(
        result.text,
        "];\n%column_names% {CONVERTER}\nmpc.convdc = ["
    )
    .unwrap();
    for c in &e.stations {
        write_converter(&mut result.text, c, bus_id(&c.dc_bus))?;
    }
    result.text.push_str("];\n");
    Ok(result)
}

fn write_converter(text: &mut String, c: &AcDcConverterStation, dc_bus: u32) -> Result<()> {
    let dc = match c.dc_control {
        DcConverterControl::ActivePowerAtPcc => 1,
        DcConverterControl::DcVoltage => 2,
        DcConverterControl::DcPowerDroop => 3,
        DcConverterControl::ValveAcPowerDroop => {
            return Err(invalid(
                "baseline output cannot express valve AC droop without Pacset",
            ));
        }
    };
    let ac = match c.ac_control {
        AcConverterControl::ReactivePowerAtPcc => 1,
        AcConverterControl::VoltageAtPcc => 2,
    };
    let s = &c.settings;
    writeln!(
        text,
        "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {};",
        dc_bus,
        c.ac_bus.0,
        dc,
        ac,
        -s.pcc_withdrawal_mw,
        -s.pcc_withdrawal_mvar,
        u8::from(c.technology == ConverterTechnology::Lcc),
        s.declared_ac_voltage_pu,
        c.transformer.resistance_pu,
        c.transformer.reactance_pu,
        u8::from(c.transformer.enabled),
        c.transformer_tap_ratio,
        c.filter_susceptance_pu,
        u8::from(c.filter_enabled),
        c.reactor.resistance_pu,
        c.reactor.reactance_pu,
        u8::from(c.reactor.enabled),
        c.ac_base_kv,
        c.valve_voltage_pu.max,
        c.valve_voltage_pu.min,
        c.valve_current_limit_pu,
        u8::from(c.in_service),
        c.losses.constant_mw,
        c.losses.linear_kv,
        c.losses.rectifier_quadratic_ohm,
        c.losses.inverter_quadratic_ohm,
        s.droop,
        s.dc_withdrawal_target_mw,
        s.dc_voltage_target_pu,
        s.dc_voltage_deadband_pu,
        c.valve_active_mw.max,
        c.valve_active_mw.min,
        c.valve_reactive_mvar.max,
        c.valve_reactive_mvar.min,
    )
    .unwrap();
    Ok(())
}

fn converter(r: &[f64], i: usize) -> Result<AcDcConverterStation> {
    let dc_control = match integer(r[2])? {
        1 => DcConverterControl::ActivePowerAtPcc,
        2 => DcConverterControl::DcVoltage,
        3 => DcConverterControl::DcPowerDroop,
        4 => {
            return Err(invalid(
                "type_dc=4 requires an explicit Pacset extension; baseline profile does not provide it",
            ));
        }
        _ => return Err(invalid("unsupported DC control code")),
    };
    let ac_control = match integer(r[3])? {
        1 => AcConverterControl::ReactivePowerAtPcc,
        2 => AcConverterControl::VoltageAtPcc,
        _ => return Err(invalid("unsupported AC control code")),
    };
    Ok(AcDcConverterStation {
        component: id("ac_dc_converter_station", i + 1),
        dc_bus: id("dc_equivalent_bus", integer(r[0])?),
        ac_bus: BusId(integer(r[1])? as usize),
        dc_control,
        ac_control,
        technology: if flag(r[6])? {
            ConverterTechnology::Lcc
        } else {
            ConverterTechnology::Vsc
        },
        settings: ConverterSettings {
            pcc_withdrawal_mw: -r[4],
            pcc_withdrawal_mvar: -r[5],
            declared_ac_voltage_pu: r[7],
            droop: r[26],
            dc_withdrawal_target_mw: r[27],
            dc_voltage_target_pu: r[28],
            dc_voltage_deadband_pu: r[29],
        },
        transformer: ConverterSeries {
            enabled: flag(r[10])?,
            resistance_pu: r[8],
            reactance_pu: r[9],
        },
        transformer_tap_ratio: r[11],
        filter_susceptance_pu: r[12],
        filter_enabled: flag(r[13])?,
        reactor: ConverterSeries {
            enabled: flag(r[16])?,
            resistance_pu: r[14],
            reactance_pu: r[15],
        },
        ac_base_kv: r[17],
        valve_voltage_pu: limits(r[19], r[18]),
        valve_current_limit_pu: r[20],
        in_service: flag(r[21])?,
        losses: AcCurrentLosses {
            constant_mw: r[22],
            linear_kv: r[23],
            rectifier_quadratic_ohm: r[24],
            inverter_quadratic_ohm: r[25],
        },
        valve_active_mw: limits(r[31], r[30]),
        valve_reactive_mvar: limits(r[33], r[32]),
    })
}
