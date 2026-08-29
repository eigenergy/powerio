//! The language neutral SCOPF document.
//!
//! The conversion is structural: every struct that reaches the document classifies
//! each of its fields as a 0-based internal index, a renamed field (the document
//! uses Greek or uppercase names), or a value passed through unchanged. Callers
//! choose whether the declared index fields remain 0-based or are renumbered to
//! 1-based. The classification destructures the struct exhaustively, so a field
//! added in `types.rs` fails to compile until it is classified here: a new index
//! field cannot be silently missed, and a value field reusing an index name (`t`,
//! `m`, `j_ln`, ...) in another struct is never bumped.

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
    ScopfTransformerSurvivorRow, ScopfVariablePhaseRow, ScopfVariableRatioRow, ScopfViolationCost,
};
use super::{ScopfResult, ScucInputs};

pub const SCOPF_SCHEMA: &str = "powerio.scopf";

/// Index convention for ordinal fields in a serialized SCOPF document.
///
/// Source identities such as bus ids and uids are not ordinals and are never
/// changed. [`Zero`](Self::Zero) is the default for Rust, C, and Python;
/// 1-based consumers can request [`One`](Self::One) at serialization time.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum IndexBase {
    /// Preserve the instance's 0-based document-order ordinals.
    #[default]
    Zero,
    /// Add one to every declared document-order ordinal.
    One,
}

impl IndexBase {
    /// The integer written to the document's `index_base` field.
    #[must_use]
    pub const fn value(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
        }
    }

    const fn offset(self) -> u64 {
        self.value() as u64
    }
}

#[derive(Serialize)]
struct Envelope {
    schema: &'static str,
    /// The powerio release that wrote this document; see [`powerio_tx::version`].
    #[serde(rename = "powerio_version")]
    powerio_version: &'static str,
    index_base: u8,
    instance: Value,
}

/// One serialized object: the fields holding 0-based internal indices
/// and the fields renamed in the document.
trait SerializedFields: Serialize {
    /// Serialized names of the fields holding 0-based internal indices.
    /// External identity (`BusId`, `uid`) is never listed.
    const INDEX_FIELDS: &'static [&'static str] = &[];
    /// `(internal, document)` name pairs.
    const RENAMED_FIELDS: &'static [(&'static str, &'static str)] = &[];
}

/// Classify every field of one struct that reaches the document. The generated function
/// destructures the struct exhaustively, so this fails to compile whenever a
/// field is added, removed, or renamed in `types.rs` without reclassifying it.
macro_rules! serialized_fields {
    ($row:ident {
        index: [$($index:ident),* $(,)?],
        values: [$($value:ident),* $(,)?]
        $(, renamed: [$($from:ident => $to:literal),+ $(,)?])? $(,)?
    }) => {
        impl SerializedFields for $row {
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

serialized_fields!(ScopfBusRow {
    index: [],
    values: [i, uid, v_min, v_max],
});
serialized_fields!(ScopfShuntRow {
    index: [j_sh],
    values: [uid, bus, g_sh, b_sh],
});
serialized_fields!(ScopfAcLineRow {
    index: [j_ln],
    values: [
        uid, to_bus, fr_bus, c_su, c_sd, u_0, s_max, g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to
    ],
});
serialized_fields!(ScopfTransformerRow {
    index: [j_xf],
    values: [
        uid, to_bus, fr_bus, c_su, c_sd, u_0, s_max, g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to
    ],
});
serialized_fields!(ScopfDcLineRow {
    index: [j_dc],
    values: [
        uid, pdc_max, qdc_fr_min, qdc_to_min, qdc_fr_max, qdc_to_max, to_bus, fr_bus
    ],
});
serialized_fields!(ScopfVariablePhaseRow {
    index: [j_xf],
    values: [phi_min, phi_max],
});
serialized_fields!(ScopfFixedPhaseRow {
    index: [j_xf],
    values: [phi_o],
});
serialized_fields!(ScopfVariableRatioRow {
    index: [j_xf],
    values: [tau_min, tau_max],
});
serialized_fields!(ScopfFixedRatioRow {
    index: [j_xf],
    values: [tau_o],
});
serialized_fields!(ScopfDeviceRow {
    index: [j_dev, j_sdd],
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
        u_0,
        p_max,
        p_min,
        q_max,
        q_min,
        sus,
        q_bound_cap,
        q_linear_cap,
        beta_ub,
        beta_lb,
        q_0_ub,
        q_0_lb,
        beta,
        q_p0
    ],
});
serialized_fields!(ScopfActiveReserveRow {
    index: [n_p],
    values: [uid, c_rgu, c_rgd, c_scr, c_nsc, c_rru, c_rrd, p_rru_min, p_rrd_min],
    renamed: [
        sigma_rgu => "σ_rgu",
        sigma_rgd => "σ_rgd",
        sigma_scr => "σ_scr",
        sigma_nsc => "σ_nsc",
    ],
});
serialized_fields!(ScopfReactiveReserveRow {
    index: [n_q],
    values: [uid, c_qru, c_qrd, q_qru_min, q_qrd_min],
});
serialized_fields!(ScopfActiveReserveSetRow {
    index: [n_p, j_dev, j_sdd],
    values: [i, uid],
});
serialized_fields!(ScopfReactiveReserveSetRow {
    index: [n_q, j_dev, j_sdd],
    values: [i, uid],
});
serialized_fields!(ScopfLengths {
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
        k => "K",
    ],
});
serialized_fields!(ScopfViolationCost {
    index: [],
    values: [p_bus, q_bus, s, e],
});
serialized_fields!(ScopfEnergyWindowMaxPrRow {
    index: [w_en_max_pr_ind],
    values: [uid, a_en_max_start, a_en_max_end, e_max],
});
serialized_fields!(ScopfEnergyWindowMaxCsRow {
    index: [w_en_max_cs_ind],
    values: [uid, a_en_max_start, a_en_max_end, e_max],
});
serialized_fields!(ScopfEnergyWindowMinPrRow {
    index: [w_en_min_pr_ind],
    values: [uid, a_en_min_start, a_en_min_end, e_min],
});
serialized_fields!(ScopfEnergyWindowMinCsRow {
    index: [w_en_min_cs_ind],
    values: [uid, a_en_min_start, a_en_min_end, e_min],
});
serialized_fields!(ScopfEnergyWindowPeriodMaxPrRow {
    index: [w_en_max_pr_ind, t],
    values: [uid, dt],
});
serialized_fields!(ScopfEnergyWindowPeriodMaxCsRow {
    index: [w_en_max_cs_ind, t],
    values: [uid, dt],
});
serialized_fields!(ScopfEnergyWindowPeriodMinPrRow {
    index: [w_en_min_pr_ind, t],
    values: [uid, dt],
});
serialized_fields!(ScopfEnergyWindowPeriodMinCsRow {
    index: [w_en_min_cs_ind, t],
    values: [uid, dt],
});
serialized_fields!(ScopfPriceBlockRow {
    index: [flat_k, t, m],
    values: [uid, c_en, p_max],
});
serialized_fields!(ScopfAcLineSurvivorRow {
    index: [ctg, j_ln],
    values: [uid, to_bus, fr_bus, b_sr, s_max_ctg],
});
serialized_fields!(ScopfTransformerSurvivorRow {
    index: [ctg, j_xf],
    values: [uid, to_bus, fr_bus, b_sr, s_max_ctg],
});
serialized_fields!(ScopfDcContingencyFlowRow {
    index: [flat_jtk_dc, ctg, j_dc, t],
    values: [to_bus, fr_bus, dt],
});

/// Convert an internal instance to the default 0-based SCOPF document.
pub fn to_json_value(instance: &ScucInputs) -> ScopfResult<Value> {
    to_json_value_with_index_base(instance, IndexBase::Zero)
}

/// Convert an internal instance to a SCOPF document with the requested index
/// convention.
pub fn to_json_value_with_index_base(
    instance: &ScucInputs,
    index_base: IndexBase,
) -> ScopfResult<Value> {
    let ScucInputs {
        static_data,
        lengths,
        energy_windows,
        price_blocks,
        ac_contingency_survivors,
        dc_contingency_flows,
        violation_cost,
        device_class_layout,
        dt,
    } = instance;
    let mut fields = Map::new();
    fields.insert(
        "static".to_owned(),
        serialize_static(static_data, index_base)?,
    );
    fields.insert("lengths".to_owned(), serialize_object(lengths, index_base)?);
    fields.insert(
        "energy_windows".to_owned(),
        serialize_energy_windows(energy_windows, index_base)?,
    );
    fields.insert(
        "price_blocks".to_owned(),
        serialize_price_blocks(price_blocks, index_base)?,
    );
    fields.insert(
        "ac_contingency_survivors".to_owned(),
        serialize_survivors(ac_contingency_survivors, index_base)?,
    );
    fields.insert(
        "dc_contingency_flows".to_owned(),
        serialize_rows(dc_contingency_flows, index_base)?,
    );
    fields.insert(
        "violation_cost".to_owned(),
        serialize_object(violation_cost, index_base)?,
    );
    fields.insert(
        "device_class_layout".to_owned(),
        serde_json::to_value(device_class_layout)?,
    );
    fields.insert("dt".to_owned(), serde_json::to_value(dt)?);
    Ok(serde_json::to_value(Envelope {
        schema: SCOPF_SCHEMA,
        powerio_version: powerio_tx::VERSION,
        index_base: index_base.value(),
        instance: Value::Object(fields),
    })?)
}

/// Serialize an internal instance as the default 0-based SCOPF document.
pub fn to_json(instance: &ScucInputs) -> ScopfResult<String> {
    to_json_with_index_base(instance, IndexBase::Zero)
}

/// Serialize an internal instance with the requested index convention.
pub fn to_json_with_index_base(
    instance: &ScucInputs,
    index_base: IndexBase,
) -> ScopfResult<String> {
    Ok(serde_json::to_string(&to_json_value_with_index_base(
        instance, index_base,
    )?)?)
}

fn serialize_static(data: &ScopfStaticData, index_base: IndexBase) -> ScopfResult<Value> {
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
    object.insert("bus".to_owned(), serialize_rows(bus, index_base)?);
    object.insert("shunt".to_owned(), serialize_rows(shunt, index_base)?);
    object.insert(
        "acl_branch".to_owned(),
        serialize_rows(acl_branch, index_base)?,
    );
    object.insert(
        "acx_branch".to_owned(),
        serialize_rows(acx_branch, index_base)?,
    );
    object.insert("vpd".to_owned(), serialize_rows(vpd, index_base)?);
    object.insert("fpd".to_owned(), serialize_rows(fpd, index_base)?);
    object.insert("vwr".to_owned(), serialize_rows(vwr, index_base)?);
    object.insert("fwr".to_owned(), serialize_rows(fwr, index_base)?);
    object.insert(
        "dc_branch".to_owned(),
        serialize_rows(dc_branch, index_base)?,
    );
    object.insert("prod".to_owned(), serialize_rows(prod, index_base)?);
    object.insert("cons".to_owned(), serialize_rows(cons, index_base)?);
    object.insert(
        "active_reserve".to_owned(),
        serialize_rows(active_reserve, index_base)?,
    );
    object.insert(
        "reactive_reserve".to_owned(),
        serialize_rows(reactive_reserve, index_base)?,
    );
    object.insert(
        "active_reserve_set_pr".to_owned(),
        serialize_rows(active_reserve_set_pr, index_base)?,
    );
    object.insert(
        "active_reserve_set_cs".to_owned(),
        serialize_rows(active_reserve_set_cs, index_base)?,
    );
    object.insert(
        "reactive_reserve_set_pr".to_owned(),
        serialize_rows(reactive_reserve_set_pr, index_base)?,
    );
    object.insert(
        "reactive_reserve_set_cs".to_owned(),
        serialize_rows(reactive_reserve_set_cs, index_base)?,
    );
    Ok(Value::Object(object))
}

fn serialize_energy_windows(
    windows: &ScopfEnergyWindows,
    index_base: IndexBase,
) -> ScopfResult<Value> {
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
    object.insert(
        "W_en_max_pr".to_owned(),
        serialize_rows(w_en_max_pr, index_base)?,
    );
    object.insert(
        "W_en_max_cs".to_owned(),
        serialize_rows(w_en_max_cs, index_base)?,
    );
    object.insert(
        "W_en_min_pr".to_owned(),
        serialize_rows(w_en_min_pr, index_base)?,
    );
    object.insert(
        "W_en_min_cs".to_owned(),
        serialize_rows(w_en_min_cs, index_base)?,
    );
    object.insert(
        "T_w_en_max_pr".to_owned(),
        serialize_rows(t_w_en_max_pr, index_base)?,
    );
    object.insert(
        "T_w_en_max_cs".to_owned(),
        serialize_rows(t_w_en_max_cs, index_base)?,
    );
    object.insert(
        "T_w_en_min_pr".to_owned(),
        serialize_rows(t_w_en_min_pr, index_base)?,
    );
    object.insert(
        "T_w_en_min_cs".to_owned(),
        serialize_rows(t_w_en_min_cs, index_base)?,
    );
    Ok(Value::Object(object))
}

fn serialize_price_blocks(blocks: &ScopfPriceBlocks, index_base: IndexBase) -> ScopfResult<Value> {
    let ScopfPriceBlocks { producer, consumer } = blocks;
    let mut object = Map::new();
    object.insert("producer".to_owned(), serialize_rows(producer, index_base)?);
    object.insert("consumer".to_owned(), serialize_rows(consumer, index_base)?);
    Ok(Value::Object(object))
}

fn serialize_survivors(
    survivors: &ScopfAcContingencySurvivors,
    index_base: IndexBase,
) -> ScopfResult<Value> {
    let ScopfAcContingencySurvivors { ln, xf } = survivors;
    let mut object = Map::new();
    object.insert("ln".to_owned(), serialize_nested_rows(ln, index_base)?);
    object.insert("xf".to_owned(), serialize_nested_rows(xf, index_base)?);
    Ok(Value::Object(object))
}

fn serialize_rows<R: SerializedFields>(rows: &[R], index_base: IndexBase) -> ScopfResult<Value> {
    rows.iter()
        .map(|row| serialize_object(row, index_base))
        .collect::<ScopfResult<Vec<_>>>()
        .map(Value::from)
}

fn serialize_nested_rows<R: SerializedFields>(
    groups: &[Vec<R>],
    index_base: IndexBase,
) -> ScopfResult<Value> {
    groups
        .iter()
        .map(|group| serialize_rows(group, index_base))
        .collect::<ScopfResult<Vec<_>>>()
        .map(Value::from)
}

/// Serialize one struct, renumber its declared index fields, apply its
/// renames. The declared fields always exist in the serialized object (the
/// classification is compile-checked against the struct), so a miss here means
/// a `serde` attribute changed the serialized name; fail loudly.
fn serialize_object<R: SerializedFields>(row: &R, index_base: IndexBase) -> ScopfResult<Value> {
    let mut value = serde_json::to_value(row)?;
    let Some(object) = value.as_object_mut() else {
        return Err(ScopfError::invalid(
            "struct did not serialize to a JSON object",
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
        let serialized = zero_based.checked_add(index_base.offset()).ok_or_else(|| {
            ScopfError::invalid(format!("index field `{field}` overflows its JSON integer"))
        })?;
        *index = Value::from(serialized);
    }
    for &(from, to) in R::RENAMED_FIELDS {
        let renamed = object
            .remove(from)
            .ok_or_else(|| ScopfError::invalid(format!("renamed field `{from}` not serialized")))?;
        object.insert(to.to_owned(), renamed);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::any::type_name;

    use serde_json::json;

    use super::*;

    const SMALL: &str = include_str!("../../tests/data/goc3_small.json");

    fn instance_with_every_row_collection() -> ScucInputs {
        let mut source: Value = serde_json::from_str(SMALL).expect("parse fixture JSON");

        // Keep the fixture's fixed phase/ratio transformer and add one variable
        // transformer so all four transformer option row types are populated.
        let mut variable = source["network"]["two_winding_transformer"][0].clone();
        variable["uid"] = json!("xf_variable");
        variable["ta_lb"] = json!(-0.1);
        variable["ta_ub"] = json!(0.1);
        variable["tm_lb"] = json!(0.9);
        variable["tm_ub"] = json!(1.1);
        source["network"]["two_winding_transformer"]
            .as_array_mut()
            .expect("transformer rows")
            .push(variable);

        // The fixture gives energy windows only to its producer. Mirror the
        // windows onto the consumer so every producer/consumer window row type
        // is represented in this one projected instance.
        let upper = source["network"]["simple_dispatchable_device"][0]["energy_req_ub"].clone();
        let lower = source["network"]["simple_dispatchable_device"][0]["energy_req_lb"].clone();
        source["network"]["simple_dispatchable_device"][1]["energy_req_ub"] = upper;
        source["network"]["simple_dispatchable_device"][1]["energy_req_lb"] = lower;

        super::super::parse_scopf_str(
            &serde_json::to_string(&source).expect("serialize complete fixture"),
            "goc3-json",
        )
        .expect("project complete fixture")
    }

    fn assert_object_base_contract<R: SerializedFields>(row: &R) {
        let zero = serialize_object(row, IndexBase::Zero).expect("serialize base 0 row");
        let one = serialize_object(row, IndexBase::One).expect("serialize base 1 row");
        let zero = zero.as_object().expect("base 0 row object");
        let one = one.as_object().expect("base 1 row object");
        assert_eq!(zero.len(), one.len(), "{} field count", type_name::<R>());

        for (field, zero_value) in zero {
            let one_value = one
                .get(field)
                .unwrap_or_else(|| panic!("{} missing field `{field}`", type_name::<R>()));
            if R::INDEX_FIELDS.contains(&field.as_str()) {
                let zero_index = zero_value.as_u64().unwrap_or_else(|| {
                    panic!("{} index field `{field}` is not unsigned", type_name::<R>())
                });
                assert_eq!(
                    one_value.as_u64(),
                    zero_index.checked_add(1),
                    "{} index field `{field}`",
                    type_name::<R>()
                );
            } else {
                assert_eq!(
                    one_value,
                    zero_value,
                    "{} nonordinal field `{field}`",
                    type_name::<R>()
                );
            }
        }
    }

    fn assert_rows_base_contract<R: SerializedFields>(rows: &[R]) {
        assert!(!rows.is_empty(), "{} collection is empty", type_name::<R>());
        for row in rows {
            assert_object_base_contract(row);
        }
    }

    fn assert_nested_rows_base_contract<R: SerializedFields>(groups: &[Vec<R>]) {
        assert!(
            groups.iter().any(|group| !group.is_empty()),
            "{} nested collection is empty",
            type_name::<R>()
        );
        for row in groups.iter().flatten() {
            assert_object_base_contract(row);
        }
    }

    #[test]
    fn every_row_field_obeys_the_selected_index_base() {
        let instance = instance_with_every_row_collection();
        let ScucInputs {
            static_data,
            lengths,
            energy_windows,
            price_blocks,
            ac_contingency_survivors,
            dc_contingency_flows,
            violation_cost,
            device_class_layout,
            dt,
        } = &instance;
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
        } = static_data;
        assert_rows_base_contract(bus);
        assert_rows_base_contract(shunt);
        assert_rows_base_contract(acl_branch);
        assert_rows_base_contract(acx_branch);
        assert_rows_base_contract(vpd);
        assert_rows_base_contract(fpd);
        assert_rows_base_contract(vwr);
        assert_rows_base_contract(fwr);
        assert_rows_base_contract(dc_branch);
        assert_rows_base_contract(prod);
        assert_rows_base_contract(cons);
        assert_rows_base_contract(active_reserve);
        assert_rows_base_contract(reactive_reserve);
        assert_rows_base_contract(active_reserve_set_pr);
        assert_rows_base_contract(active_reserve_set_cs);
        assert_rows_base_contract(reactive_reserve_set_pr);
        assert_rows_base_contract(reactive_reserve_set_cs);

        assert_object_base_contract(lengths);
        let ScopfEnergyWindows {
            w_en_max_pr,
            w_en_max_cs,
            w_en_min_pr,
            w_en_min_cs,
            t_w_en_max_pr,
            t_w_en_max_cs,
            t_w_en_min_pr,
            t_w_en_min_cs,
        } = energy_windows;
        assert_rows_base_contract(w_en_max_pr);
        assert_rows_base_contract(w_en_max_cs);
        assert_rows_base_contract(w_en_min_pr);
        assert_rows_base_contract(w_en_min_cs);
        assert_rows_base_contract(t_w_en_max_pr);
        assert_rows_base_contract(t_w_en_max_cs);
        assert_rows_base_contract(t_w_en_min_pr);
        assert_rows_base_contract(t_w_en_min_cs);

        let ScopfPriceBlocks { producer, consumer } = price_blocks;
        assert_rows_base_contract(producer);
        assert_rows_base_contract(consumer);

        let ScopfAcContingencySurvivors { ln, xf } = ac_contingency_survivors;
        assert_nested_rows_base_contract(ln);
        assert_nested_rows_base_contract(xf);
        assert_rows_base_contract(dc_contingency_flows);
        assert_object_base_contract(violation_cost);

        let zero = to_json_value_with_index_base(&instance, IndexBase::Zero)
            .expect("serialize base 0 envelope");
        let one = to_json_value_with_index_base(&instance, IndexBase::One)
            .expect("serialize base 1 envelope");
        assert_eq!(zero["schema"], one["schema"]);
        assert_eq!(zero["powerio_version"], one["powerio_version"]);
        assert_eq!(
            zero["instance"]["device_class_layout"],
            json!(device_class_layout)
        );
        assert_eq!(
            one["instance"]["device_class_layout"],
            json!(device_class_layout)
        );
        assert_eq!(zero["instance"]["dt"], json!(dt));
        assert_eq!(one["instance"]["dt"], json!(dt));
        assert_eq!(zero["index_base"], 0);
        assert_eq!(one["index_base"], 1);
    }
}
