//! Versioned Julia compatibility wire format.
//!
//! The conversion is structural: every struct that reaches the wire classifies
//! each of its fields as a 0-based internal index (renumbered to 1-based), a
//! renamed field (Julia spells some names in Greek or uppercase), or a value
//! passed through unchanged. The classification destructures the struct
//! exhaustively, so a field added in `types.rs` fails to compile until it is
//! classified here: a new index field cannot be silently missed, and a value
//! field reusing an index name (`t`, `m`, `j_ln`, ...) in another struct is
//! never bumped.

use serde::Serialize;
use serde_json::{Map, Value};

use super::error::ScopfError;
use super::types::{
    ScopfAcContingencySurvivors, ScopfAcLineRow, ScopfAcLineSurvivorRow, ScopfActiveReserveRow,
    ScopfActiveReserveSetRow, ScopfBusRow, ScopfDcContingencyFlowRow, ScopfDcLineRow,
    ScopfDeviceRow, ScopfEnergyWindowMaxCsRow, ScopfEnergyWindowMaxPrRow,
    ScopfEnergyWindowMinCsRow, ScopfEnergyWindowMinPrRow, ScopfEnergyWindowPeriodMaxCsRow,
    ScopfEnergyWindowPeriodMaxPrRow, ScopfEnergyWindowPeriodMinCsRow,
    ScopfEnergyWindowPeriodMinPrRow, ScopfEnergyWindows, ScopfFixedPhaseRow, ScopfFixedRatioRow,
    ScopfLengths, ScopfPriceBlockRow, ScopfPriceBlocks, ScopfReactiveReserveRow,
    ScopfReactiveReserveSetRow, ScopfShuntRow, ScopfStaticData, ScopfTransformerRow,
    ScopfTransformerSurvivorRow, ScopfVariablePhaseRow, ScopfVariableRatioRow,
};
use super::{ScopfInstance, ScopfResult};

pub const SCOPF_WIRE_SCHEMA: &str = "powerio.scopf.julia";
pub const SCOPF_WIRE_VERSION: &str = "1.0.0";

#[derive(Serialize)]
struct WireEnvelope {
    schema: &'static str,
    schema_version: &'static str,
    index_base: usize,
    instance: Value,
}

/// Wire conversion of one serialized object: the fields holding 0-based
/// internal indices and the fields renamed on the wire.
trait WireFields: Serialize {
    /// Serialized names of the fields holding 0-based internal indices.
    /// External identity (`BusId`, `uid`) is never listed.
    const INDEX_FIELDS: &'static [&'static str] = &[];
    /// `(internal, wire)` name pairs.
    const RENAMED_FIELDS: &'static [(&'static str, &'static str)] = &[];
}

/// Classify every field of one struct that reaches the wire. The generated function
/// destructures the struct exhaustively, so this fails to compile whenever a
/// field is added, removed, or renamed in `types.rs` without reclassifying it.
macro_rules! wire_fields {
    ($row:ident {
        index: [$($index:ident),* $(,)?],
        values: [$($value:ident),* $(,)?]
        $(, renamed: [$($from:ident => $to:literal),+ $(,)?])? $(,)?
    }) => {
        impl WireFields for $row {
            const INDEX_FIELDS: &'static [&'static str] = &[$(stringify!($index)),*];
            $(const RENAMED_FIELDS: &'static [(&'static str, &'static str)] =
                &[$((stringify!($from), $to)),+];)?
        }
        const _: () = {
            #[allow(dead_code)]
            fn classified(row: $row) {
                let $row { $($index: _,)* $($value: _,)* $($($from: _,)+)? } = row;
            }
        };
    };
}

wire_fields!(ScopfBusRow {
    index: [],
    values: [i, uid, v_min, v_max],
});
wire_fields!(ScopfShuntRow {
    index: [],
    values: [uid, bus, g_sh, b_sh],
});
wire_fields!(ScopfAcLineRow {
    index: [j_ln],
    values: [
        uid, to_bus, fr_bus, c_su, c_sd, s_max, g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to
    ],
});
wire_fields!(ScopfTransformerRow {
    index: [j_xf],
    values: [
        uid, to_bus, fr_bus, c_su, c_sd, s_max, g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to
    ],
});
wire_fields!(ScopfDcLineRow {
    index: [j_dc],
    values: [
        uid, pdc_max, qdc_fr_min, qdc_to_min, qdc_fr_max, qdc_to_max, to_bus, fr_bus
    ],
});
wire_fields!(ScopfVariablePhaseRow {
    index: [j_xf],
    values: [phi_min, phi_max],
});
wire_fields!(ScopfFixedPhaseRow {
    index: [j_xf],
    values: [phi_o],
});
wire_fields!(ScopfVariableRatioRow {
    index: [j_xf],
    values: [tau_min, tau_max],
});
wire_fields!(ScopfFixedRatioRow {
    index: [j_xf],
    values: [tau_o],
});
wire_fields!(ScopfDeviceRow {
    index: [],
    values: [
        bus,
        uid,
        c_on,
        c_su,
        c_sd,
        p_ru,
        p_rd,
        p_ru_su,
        p_rd_sd,
        c_rgu,
        c_rgd,
        c_scr,
        c_nsc,
        c_rru_on,
        c_rru_off,
        c_rrd_on,
        c_rrd_off,
        c_qru,
        c_qrd,
        p_rgu_max,
        p_rgd_max,
        p_scr_max,
        p_nsc_max,
        p_rru_on_max,
        p_rru_off_max,
        p_rrd_on_max,
        p_rrd_off_max,
        p_0,
        q_0,
        p_max,
        p_min,
        q_max,
        q_min,
        sus
    ],
});
wire_fields!(ScopfActiveReserveRow {
    index: [n_p],
    values: [uid, c_rgu, c_rgd, c_scr, c_nsc, c_rru, c_rrd, p_rru_min, p_rrd_min],
    renamed: [
        sigma_rgu => "σ_rgu",
        sigma_rgd => "σ_rgd",
        sigma_scr => "σ_scr",
        sigma_nsc => "σ_nsc",
    ],
});
wire_fields!(ScopfReactiveReserveRow {
    index: [n_q],
    values: [uid, c_qru, c_qrd, q_qru_min, q_qrd_min],
});
wire_fields!(ScopfActiveReserveSetRow {
    index: [n_p],
    values: [i, uid],
});
wire_fields!(ScopfReactiveReserveSetRow {
    index: [n_q],
    values: [i, uid],
});
wire_fields!(ScopfLengths {
    index: [],
    values: [],
    renamed: [
        l_j_xf => "L_J_xf",
        l_j_ln => "L_J_ln",
        l_j_ac => "L_J_ac",
        l_j_dc => "L_J_dc",
        l_j_br => "L_J_br",
        l_j_cs => "L_J_cs",
        l_j_pr => "L_J_pr",
        l_j_cspr => "L_J_cspr",
        l_j_sh => "L_J_sh",
        i => "I",
        l_t => "L_T",
        l_n_p => "L_N_p",
        l_n_q => "L_N_q",
    ],
});
wire_fields!(ScopfEnergyWindowMaxPrRow {
    index: [w_en_max_pr_ind],
    values: [uid, a_en_max_start, a_en_max_end, e_max],
});
wire_fields!(ScopfEnergyWindowMaxCsRow {
    index: [w_en_max_cs_ind],
    values: [uid, a_en_max_start, a_en_max_end, e_max],
});
wire_fields!(ScopfEnergyWindowMinPrRow {
    index: [w_en_min_pr_ind],
    values: [uid, a_en_min_start, a_en_min_end, e_min],
});
wire_fields!(ScopfEnergyWindowMinCsRow {
    index: [w_en_min_cs_ind],
    values: [uid, a_en_min_start, a_en_min_end, e_min],
});
wire_fields!(ScopfEnergyWindowPeriodMaxPrRow {
    index: [w_en_max_pr_ind, t],
    values: [uid, dt],
});
wire_fields!(ScopfEnergyWindowPeriodMaxCsRow {
    index: [w_en_max_cs_ind, t],
    values: [uid, dt],
});
wire_fields!(ScopfEnergyWindowPeriodMinPrRow {
    index: [w_en_min_pr_ind, t],
    values: [uid, dt],
});
wire_fields!(ScopfEnergyWindowPeriodMinCsRow {
    index: [w_en_min_cs_ind, t],
    values: [uid, dt],
});
wire_fields!(ScopfPriceBlockRow {
    index: [flat_k, t, m],
    values: [uid, c_en, p_max],
});
wire_fields!(ScopfAcLineSurvivorRow {
    index: [ctg, j_ln],
    values: [uid, to_bus, fr_bus, b_sr, s_max_ctg],
});
wire_fields!(ScopfTransformerSurvivorRow {
    index: [ctg, j_xf],
    values: [uid, to_bus, fr_bus, b_sr, s_max_ctg],
});
wire_fields!(ScopfDcContingencyFlowRow {
    index: [flat_jtk_dc, ctg, j_dc, t],
    values: [to_bus, fr_bus, dt],
});

/// Convert an internal instance to the versioned 1-based Julia wire format.
pub fn to_wire_value(instance: &ScopfInstance) -> ScopfResult<Value> {
    let ScopfInstance {
        static_data,
        lengths,
        energy_windows,
        price_blocks,
        ac_contingency_survivors,
        dc_contingency_flows,
    } = instance;
    let mut wire = Map::new();
    wire.insert("static".to_owned(), wire_static(static_data)?);
    wire.insert("lengths".to_owned(), wire_object(lengths)?);
    wire.insert(
        "energy_windows".to_owned(),
        wire_energy_windows(energy_windows)?,
    );
    wire.insert("price_blocks".to_owned(), wire_price_blocks(price_blocks)?);
    wire.insert(
        "ac_contingency_survivors".to_owned(),
        wire_survivors(ac_contingency_survivors)?,
    );
    wire.insert(
        "dc_contingency_flows".to_owned(),
        wire_rows(dc_contingency_flows)?,
    );
    Ok(serde_json::to_value(WireEnvelope {
        schema: SCOPF_WIRE_SCHEMA,
        schema_version: SCOPF_WIRE_VERSION,
        index_base: 1,
        instance: Value::Object(wire),
    })?)
}

/// Serialize an internal instance as the versioned 1-based Julia wire format.
pub fn to_wire_json(instance: &ScopfInstance) -> ScopfResult<String> {
    Ok(serde_json::to_string(&to_wire_value(instance)?)?)
}

fn wire_static(data: &ScopfStaticData) -> ScopfResult<Value> {
    let ScopfStaticData {
        bus,
        shunt,
        acl_branch,
        acx_branch,
        vpd,
        fpd,
        vwr,
        fwr,
        dc_branch,
        prod,
        cons,
        active_reserve,
        reactive_reserve,
        active_reserve_set_pr,
        active_reserve_set_cs,
        reactive_reserve_set_pr,
        reactive_reserve_set_cs,
    } = data;
    let mut object = Map::new();
    object.insert("bus".to_owned(), wire_rows(bus)?);
    object.insert("shunt".to_owned(), wire_rows(shunt)?);
    object.insert("acl_branch".to_owned(), wire_rows(acl_branch)?);
    object.insert("acx_branch".to_owned(), wire_rows(acx_branch)?);
    object.insert("vpd".to_owned(), wire_rows(vpd)?);
    object.insert("fpd".to_owned(), wire_rows(fpd)?);
    object.insert("vwr".to_owned(), wire_rows(vwr)?);
    object.insert("fwr".to_owned(), wire_rows(fwr)?);
    object.insert("dc_branch".to_owned(), wire_rows(dc_branch)?);
    object.insert("prod".to_owned(), wire_rows(prod)?);
    object.insert("cons".to_owned(), wire_rows(cons)?);
    object.insert("active_reserve".to_owned(), wire_rows(active_reserve)?);
    object.insert("reactive_reserve".to_owned(), wire_rows(reactive_reserve)?);
    object.insert(
        "active_reserve_set_pr".to_owned(),
        wire_rows(active_reserve_set_pr)?,
    );
    object.insert(
        "active_reserve_set_cs".to_owned(),
        wire_rows(active_reserve_set_cs)?,
    );
    object.insert(
        "reactive_reserve_set_pr".to_owned(),
        wire_rows(reactive_reserve_set_pr)?,
    );
    object.insert(
        "reactive_reserve_set_cs".to_owned(),
        wire_rows(reactive_reserve_set_cs)?,
    );
    Ok(Value::Object(object))
}

fn wire_energy_windows(windows: &ScopfEnergyWindows) -> ScopfResult<Value> {
    let ScopfEnergyWindows {
        w_en_max_pr,
        w_en_max_cs,
        w_en_min_pr,
        w_en_min_cs,
        t_w_en_max_pr,
        t_w_en_max_cs,
        t_w_en_min_pr,
        t_w_en_min_cs,
    } = windows;
    let mut object = Map::new();
    object.insert("W_en_max_pr".to_owned(), wire_rows(w_en_max_pr)?);
    object.insert("W_en_max_cs".to_owned(), wire_rows(w_en_max_cs)?);
    object.insert("W_en_min_pr".to_owned(), wire_rows(w_en_min_pr)?);
    object.insert("W_en_min_cs".to_owned(), wire_rows(w_en_min_cs)?);
    object.insert("T_w_en_max_pr".to_owned(), wire_rows(t_w_en_max_pr)?);
    object.insert("T_w_en_max_cs".to_owned(), wire_rows(t_w_en_max_cs)?);
    object.insert("T_w_en_min_pr".to_owned(), wire_rows(t_w_en_min_pr)?);
    object.insert("T_w_en_min_cs".to_owned(), wire_rows(t_w_en_min_cs)?);
    Ok(Value::Object(object))
}

fn wire_price_blocks(blocks: &ScopfPriceBlocks) -> ScopfResult<Value> {
    let ScopfPriceBlocks { producer, consumer } = blocks;
    let mut object = Map::new();
    object.insert("producer".to_owned(), wire_rows(producer)?);
    object.insert("consumer".to_owned(), wire_rows(consumer)?);
    Ok(Value::Object(object))
}

fn wire_survivors(survivors: &ScopfAcContingencySurvivors) -> ScopfResult<Value> {
    let ScopfAcContingencySurvivors { ln, xf } = survivors;
    let mut object = Map::new();
    object.insert("ln".to_owned(), wire_nested_rows(ln)?);
    object.insert("xf".to_owned(), wire_nested_rows(xf)?);
    Ok(Value::Object(object))
}

fn wire_rows<R: WireFields>(rows: &[R]) -> ScopfResult<Value> {
    rows.iter()
        .map(wire_object)
        .collect::<ScopfResult<Vec<_>>>()
        .map(Value::from)
}

fn wire_nested_rows<R: WireFields>(groups: &[Vec<R>]) -> ScopfResult<Value> {
    groups
        .iter()
        .map(|group| wire_rows(group))
        .collect::<ScopfResult<Vec<_>>>()
        .map(Value::from)
}

/// Serialize one struct, renumber its declared index fields, apply its wire
/// renames. The declared fields always exist in the serialized object (the
/// classification is compile-checked against the struct), so a miss here means
/// a `serde` attribute changed the serialized name; fail loudly.
fn wire_object<R: WireFields>(row: &R) -> ScopfResult<Value> {
    let mut value = serde_json::to_value(row)?;
    let Some(object) = value.as_object_mut() else {
        return Err(ScopfError::invalid(
            "wire struct did not serialize to a JSON object",
        ));
    };
    for &field in R::INDEX_FIELDS {
        let index = object
            .get_mut(field)
            .ok_or_else(|| ScopfError::invalid(format!("index field `{field}` not serialized")))?;
        let Some(zero_based) = index.as_u64() else {
            return Err(ScopfError::invalid(format!(
                "index field `{field}` is not an unsigned integer"
            )));
        };
        *index = Value::from(zero_based + 1);
    }
    for &(from, to) in R::RENAMED_FIELDS {
        let renamed = object
            .remove(from)
            .ok_or_else(|| ScopfError::invalid(format!("renamed field `{from}` not serialized")))?;
        object.insert(to.to_owned(), renamed);
    }
    Ok(value)
}
