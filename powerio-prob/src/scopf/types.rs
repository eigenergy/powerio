use powerio::BusId;
use serde::{Deserialize, Serialize};

/// One bus row: `(i, uid, v_min, v_max)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfBusRow {
    pub i: BusId,
    pub uid: String,
    pub v_min: f64,
    pub v_max: f64,
}

/// One shunt row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfShuntRow {
    pub uid: String,
    pub bus: BusId,
    pub g_sh: f64,
    pub b_sh: f64,
}

/// One AC line row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfAcLineRow {
    pub j_ln: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub c_su: f64,
    pub c_sd: f64,
    pub s_max: f64,
    pub g_sr: f64,
    pub b_sr: f64,
    pub b_ch: f64,
    pub g_fr: f64,
    pub g_to: f64,
    pub b_fr: f64,
    pub b_to: f64,
}

/// One two winding transformer row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfTransformerRow {
    pub j_xf: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub c_su: f64,
    pub c_sd: f64,
    pub s_max: f64,
    pub g_sr: f64,
    pub b_sr: f64,
    pub b_ch: f64,
    pub g_fr: f64,
    pub g_to: f64,
    pub b_fr: f64,
    pub b_to: f64,
}

/// One DC line row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfDcLineRow {
    pub j_dc: usize,
    pub uid: String,
    pub pdc_max: f64,
    pub qdc_fr_min: f64,
    pub qdc_to_min: f64,
    pub qdc_fr_max: f64,
    pub qdc_to_max: f64,
    pub to_bus: BusId,
    pub fr_bus: BusId,
}

/// A transformer with a variable phase-shift control range (`vpd`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfVariablePhaseRow {
    pub j_xf: usize,
    pub phi_min: f64,
    pub phi_max: f64,
}

/// A transformer with a fixed phase shift (`fpd`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfFixedPhaseRow {
    pub j_xf: usize,
    pub phi_o: f64,
}

/// A transformer with a variable winding ratio control range (`vwr`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfVariableRatioRow {
    pub j_xf: usize,
    pub tau_min: f64,
    pub tau_max: f64,
}

/// A transformer with a fixed winding ratio (`fwr`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfFixedRatioRow {
    pub j_xf: usize,
    pub tau_o: f64,
}

/// One simple dispatchable device row.
///
/// Producers and consumers share this layout. Vector fields use the internal
/// zero based period order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfDeviceRow {
    pub bus: BusId,
    pub uid: String,
    pub c_on: f64,
    pub c_su: f64,
    pub c_sd: f64,
    pub p_ru: f64,
    pub p_rd: f64,
    pub p_ru_su: f64,
    pub p_rd_sd: f64,
    pub c_rgu: Vec<f64>,
    pub c_rgd: Vec<f64>,
    pub c_scr: Vec<f64>,
    pub c_nsc: Vec<f64>,
    pub c_rru_on: Vec<f64>,
    pub c_rru_off: Vec<f64>,
    pub c_rrd_on: Vec<f64>,
    pub c_rrd_off: Vec<f64>,
    pub c_qru: Vec<f64>,
    pub c_qrd: Vec<f64>,
    pub p_rgu_max: f64,
    pub p_rgd_max: f64,
    pub p_scr_max: f64,
    pub p_nsc_max: f64,
    pub p_rru_on_max: f64,
    pub p_rru_off_max: f64,
    pub p_rrd_on_max: f64,
    pub p_rrd_off_max: f64,
    pub p_0: f64,
    pub q_0: f64,
    pub p_max: Vec<f64>,
    pub p_min: Vec<f64>,
    pub q_max: Vec<f64>,
    pub q_min: Vec<f64>,
    pub sus: Vec<Vec<f64>>,
}

/// One active power zonal reserve row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfActiveReserveRow {
    pub n_p: usize,
    pub uid: String,
    pub c_rgu: f64,
    pub c_rgd: f64,
    pub c_scr: f64,
    pub c_nsc: f64,
    pub c_rru: f64,
    pub c_rrd: f64,
    pub sigma_rgu: f64,
    pub sigma_rgd: f64,
    pub sigma_scr: f64,
    pub sigma_nsc: f64,
    pub p_rru_min: Vec<f64>,
    pub p_rrd_min: Vec<f64>,
}

/// One reactive (reactive-power) zonal reserve row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfReactiveReserveRow {
    pub n_q: usize,
    pub uid: String,
    pub c_qru: f64,
    pub c_qrd: f64,
    pub q_qru_min: Vec<f64>,
    pub q_qrd_min: Vec<f64>,
}

/// One (bus, active reserve zone, device) membership row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfActiveReserveSetRow {
    pub i: BusId,
    pub n_p: usize,
    pub uid: String,
}

/// One (bus, reactive reserve zone, device) membership row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfReactiveReserveSetRow {
    pub i: BusId,
    pub n_q: usize,
    pub uid: String,
}

/// Set sizes for each indexed device class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfLengths {
    pub l_j_xf: usize,
    pub l_j_ln: usize,
    pub l_j_ac: usize,
    pub l_j_dc: usize,
    pub l_j_br: usize,
    pub l_j_cs: usize,
    pub l_j_pr: usize,
    pub l_j_cspr: usize,
    pub l_j_sh: usize,
    /// Bus count.
    pub i: usize,
    pub l_t: usize,
    pub l_n_p: usize,
    pub l_n_q: usize,
}

/// Static buses, branches, devices, controls, reserves, and memberships.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfStaticData {
    pub bus: Vec<ScopfBusRow>,
    pub shunt: Vec<ScopfShuntRow>,
    pub acl_branch: Vec<ScopfAcLineRow>,
    pub acx_branch: Vec<ScopfTransformerRow>,
    pub vpd: Vec<ScopfVariablePhaseRow>,
    pub fpd: Vec<ScopfFixedPhaseRow>,
    pub vwr: Vec<ScopfVariableRatioRow>,
    pub fwr: Vec<ScopfFixedRatioRow>,
    pub dc_branch: Vec<ScopfDcLineRow>,
    pub prod: Vec<ScopfDeviceRow>,
    pub cons: Vec<ScopfDeviceRow>,
    pub active_reserve: Vec<ScopfActiveReserveRow>,
    pub reactive_reserve: Vec<ScopfReactiveReserveRow>,
    pub active_reserve_set_pr: Vec<ScopfActiveReserveSetRow>,
    pub active_reserve_set_cs: Vec<ScopfActiveReserveSetRow>,
    pub reactive_reserve_set_pr: Vec<ScopfReactiveReserveSetRow>,
    pub reactive_reserve_set_cs: Vec<ScopfReactiveReserveSetRow>,
}

/// One device energy cost curve. `cost[t][m]` is `[c_en, p_max]` for price
/// block `m` in period `t`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ScopfCostRow {
    pub(super) bus: BusId,
    pub(super) uid: String,
    pub(super) cost: Vec<Vec<[f64; 2]>>,
}

/// Static index sets and the cost vectors used by the price block projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub(super) struct ScopfStaticDataProjection {
    pub(super) static_data: ScopfStaticData,
    pub(super) lengths: ScopfLengths,
    pub(super) cost_vector_pr: Vec<ScopfCostRow>,
    pub(super) cost_vector_cs: Vec<ScopfCostRow>,
}

macro_rules! energy_window_row {
    ($name:ident, $ind_field:ident, $start_field:ident, $end_field:ident, $bound_field:ident) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        pub struct $name {
            pub $ind_field: usize,
            pub uid: String,
            pub $start_field: f64,
            pub $end_field: f64,
            pub $bound_field: f64,
        }
    };
}

energy_window_row!(
    ScopfEnergyWindowMaxPrRow,
    w_en_max_pr_ind,
    a_en_max_start,
    a_en_max_end,
    e_max
);
energy_window_row!(
    ScopfEnergyWindowMaxCsRow,
    w_en_max_cs_ind,
    a_en_max_start,
    a_en_max_end,
    e_max
);
energy_window_row!(
    ScopfEnergyWindowMinPrRow,
    w_en_min_pr_ind,
    a_en_min_start,
    a_en_min_end,
    e_min
);
energy_window_row!(
    ScopfEnergyWindowMinCsRow,
    w_en_min_cs_ind,
    a_en_min_start,
    a_en_min_end,
    e_min
);

macro_rules! energy_window_period_row {
    ($name:ident, $ind_field:ident) => {
        /// Period membership of one energy window: the period belongs when
        /// its midpoint falls within the window's `(start, end]` interval.
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        pub struct $name {
            pub $ind_field: usize,
            pub uid: String,
            pub t: usize,
            pub dt: f64,
        }
    };
}

energy_window_period_row!(ScopfEnergyWindowPeriodMaxPrRow, w_en_max_pr_ind);
energy_window_period_row!(ScopfEnergyWindowPeriodMaxCsRow, w_en_max_cs_ind);
energy_window_period_row!(ScopfEnergyWindowPeriodMinPrRow, w_en_min_pr_ind);
energy_window_period_row!(ScopfEnergyWindowPeriodMinCsRow, w_en_min_cs_ind);

/// Energy requirement windows and their period memberships.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfEnergyWindows {
    pub w_en_max_pr: Vec<ScopfEnergyWindowMaxPrRow>,
    pub w_en_max_cs: Vec<ScopfEnergyWindowMaxCsRow>,
    pub w_en_min_pr: Vec<ScopfEnergyWindowMinPrRow>,
    pub w_en_min_cs: Vec<ScopfEnergyWindowMinCsRow>,
    pub t_w_en_max_pr: Vec<ScopfEnergyWindowPeriodMaxPrRow>,
    pub t_w_en_max_cs: Vec<ScopfEnergyWindowPeriodMaxCsRow>,
    pub t_w_en_min_pr: Vec<ScopfEnergyWindowPeriodMinPrRow>,
    pub t_w_en_min_cs: Vec<ScopfEnergyWindowPeriodMinCsRow>,
}

/// One flattened device, period, and price block row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfPriceBlockRow {
    pub flat_k: usize,
    pub uid: String,
    pub t: usize,
    pub m: usize,
    pub c_en: f64,
    pub p_max: f64,
}

/// Flattened producer and consumer price blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfPriceBlocks {
    pub producer: Vec<ScopfPriceBlockRow>,
    pub consumer: Vec<ScopfPriceBlockRow>,
}

/// One AC line surviving a contingency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfAcLineSurvivorRow {
    pub ctg: usize,
    pub j_ln: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub b_sr: f64,
    pub s_max_ctg: f64,
}

/// One transformer surviving a contingency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfTransformerSurvivorRow {
    pub ctg: usize,
    pub j_xf: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub b_sr: f64,
    pub s_max_ctg: f64,
}

/// Per-contingency surviving AC lines and transformers, one group per
/// contingency in `reliability.contingency` document order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfAcContingencySurvivors {
    pub ln: Vec<Vec<ScopfAcLineSurvivorRow>>,
    pub xf: Vec<Vec<ScopfTransformerSurvivorRow>>,
}

/// One surviving DC line in one contingency and period.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfDcContingencyFlowRow {
    pub flat_jtk_dc: usize,
    pub ctg: usize,
    pub j_dc: usize,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub t: usize,
    pub dt: f64,
}

/// Matrix free SCOPF input data.
///
/// Internal class, period, contingency, window, and flattened row indices are
/// zero based. Source UIDs and external bus IDs remain separate fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScopfInstance {
    /// Buses, branches, devices, controls, reserves, and memberships.
    pub static_data: ScopfStaticData,
    pub lengths: ScopfLengths,
    pub energy_windows: ScopfEnergyWindows,
    pub price_blocks: ScopfPriceBlocks,
    pub ac_contingency_survivors: ScopfAcContingencySurvivors,
    pub dc_contingency_flows: Vec<ScopfDcContingencyFlowRow>,
}
