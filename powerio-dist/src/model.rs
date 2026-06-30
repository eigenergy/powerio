//! The canonical multiconductor network model.
//!
//! Wire coordinates with BMOPF semantics: string bus ids, ordered string
//! terminal names per bus, explicit grounding on buses, terminal maps on
//! every element, SI units (V, W, var, ohm, S, meters) and radians. Terminal
//! names are the OpenDSS node numbers as strings; implicit ground
//! connections materialize as an explicit perfectly grounded neutral
//! terminal on the bus (named 4 on a three phase bus), the convention
//! PowerModelsDistribution and the public BMOPF examples share.
//!
//! Transformer impedances stay in the per unit form the source formats use
//! (`r_pct`, `xsc_pct` as percent of the winding base); the BMOPF writer
//! converts to ohms on the wye side at emission. Everything an element
//! carries beyond the typed fields lives in its `extras` map.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub type Extras = BTreeMap<String, serde_json::Value>;

/// A square matrix in conductor order, row major.
pub type Mat = Vec<Vec<f64>>;

/// Where the network came from; fixes the echo tier target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DistSourceFormat {
    Dss,
    BmopfJson,
    PmdJson,
}

impl DistSourceFormat {
    /// The canonical format name (`dss`, `pmd-json`, `bmopf-json`), accepted
    /// back by [`crate::dist_target_from_name`].
    pub fn name(self) -> &'static str {
        match self {
            DistSourceFormat::Dss => "dss",
            DistSourceFormat::PmdJson => "pmd-json",
            DistSourceFormat::BmopfJson => "bmopf-json",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistBus {
    pub id: String,
    /// Ordered terminal names; OpenDSS node numbers as strings.
    pub terminals: Vec<String>,
    /// Terminals tied to ground with zero impedance.
    pub grounded: Vec<String>,
    /// Voltage magnitude bounds, volts: the scalar pair plus the phase to
    /// neutral, phase to phase, and symmetrical component families (the
    /// four BMOPF bound families).
    pub v_min: Option<f64>,
    pub v_max: Option<f64>,
    pub vpn_min: Option<Vec<f64>>,
    pub vpn_max: Option<Vec<f64>>,
    pub vpp_min: Option<Vec<f64>>,
    pub vpp_max: Option<Vec<f64>>,
    pub vsym_min: Option<Vec<f64>>,
    pub vsym_max: Option<Vec<f64>>,
    pub extras: Extras,
}

impl DistBus {
    #[must_use]
    pub fn new(id: impl Into<String>, terminals: Vec<String>) -> Self {
        Self {
            id: id.into(),
            terminals,
            grounded: Vec::new(),
            v_min: None,
            v_max: None,
            vpn_min: None,
            vpn_max: None,
            vpp_min: None,
            vpp_max: None,
            vsym_min: None,
            vsym_max: None,
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistLineCode {
    pub name: String,
    pub n_conductors: usize,
    /// Series impedance, ohm per meter.
    pub r_series: Mat,
    pub x_series: Mat,
    /// Shunt admittance halves at each end, S per meter.
    pub g_from: Mat,
    pub b_from: Mat,
    pub g_to: Mat,
    pub b_to: Mat,
    /// Ampacity per conductor.
    pub i_max: Option<Vec<f64>>,
    pub s_max: Option<Vec<f64>>,
    pub extras: Extras,
}

impl DistLineCode {
    #[must_use]
    pub fn new(name: impl Into<String>, r_series: Mat, x_series: Mat) -> Self {
        let n_conductors = r_series.len();
        Self {
            name: name.into(),
            n_conductors,
            r_series,
            x_series,
            g_from: zero_mat(n_conductors),
            b_from: zero_mat(n_conductors),
            g_to: zero_mat(n_conductors),
            b_to: zero_mat(n_conductors),
            i_max: None,
            s_max: None,
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistLine {
    pub name: String,
    pub bus_from: String,
    pub bus_to: String,
    pub terminal_map_from: Vec<String>,
    pub terminal_map_to: Vec<String>,
    pub linecode: String,
    /// Meters.
    pub length: f64,
    pub extras: Extras,
}

impl DistLine {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        bus_from: impl Into<String>,
        bus_to: impl Into<String>,
        terminal_map_from: Vec<String>,
        terminal_map_to: Vec<String>,
        linecode: impl Into<String>,
        length: f64,
    ) -> Self {
        Self {
            name: name.into(),
            bus_from: bus_from.into(),
            bus_to: bus_to.into(),
            terminal_map_from,
            terminal_map_to,
            linecode: linecode.into(),
            length,
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistSwitch {
    pub name: String,
    pub bus_from: String,
    pub bus_to: String,
    pub terminal_map_from: Vec<String>,
    pub terminal_map_to: Vec<String>,
    pub open: bool,
    pub i_max: Option<Vec<f64>>,
    pub extras: Extras,
}

impl DistSwitch {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        bus_from: impl Into<String>,
        bus_to: impl Into<String>,
        terminal_map_from: Vec<String>,
        terminal_map_to: Vec<String>,
        open: bool,
    ) -> Self {
        Self {
            name: name.into(),
            bus_from: bus_from.into(),
            bus_to: bus_to.into(),
            terminal_map_from,
            terminal_map_to,
            open,
            i_max: None,
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Configuration {
    Wye,
    Delta,
    SinglePhase,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistLoad {
    pub name: String,
    pub bus: String,
    pub terminal_map: Vec<String>,
    pub configuration: Configuration,
    /// Watts per phase.
    pub p_nom: Vec<f64>,
    /// Vars per phase.
    pub q_nom: Vec<f64>,
    pub voltage_model: DistLoadVoltageModel,
    pub extras: Extras,
}

impl DistLoad {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        bus: impl Into<String>,
        terminal_map: Vec<String>,
        configuration: Configuration,
        p_nom: Vec<f64>,
        q_nom: Vec<f64>,
    ) -> Self {
        Self {
            name: name.into(),
            bus: bus.into(),
            terminal_map,
            configuration,
            p_nom,
            q_nom,
            voltage_model: DistLoadVoltageModel::default(),
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "model", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DistLoadVoltageModel {
    /// Constant power load. `v_nom` is volts per active phase when the source
    /// states it.
    ConstantPower { v_nom: Vec<f64> },
    /// Constant current load. `v_nom` is volts per active phase.
    ConstantCurrent { v_nom: Vec<f64> },
    /// Constant impedance load. `v_nom` is volts per active phase.
    ConstantImpedance { v_nom: Vec<f64> },
    /// ZIP load coefficients by active phase. `v_nom` is volts per active
    /// phase; alpha terms apply to active power and beta terms to reactive
    /// power.
    Zip {
        v_nom: Vec<f64>,
        alpha_z: Vec<f64>,
        alpha_i: Vec<f64>,
        alpha_p: Vec<f64>,
        beta_z: Vec<f64>,
        beta_i: Vec<f64>,
        beta_p: Vec<f64>,
    },
    /// Exponential voltage model by active phase. `v_nom` is volts per active
    /// phase.
    Exponential {
        v_nom: Vec<f64>,
        gamma_p: Vec<f64>,
        gamma_q: Vec<f64>,
    },
}

impl Default for DistLoadVoltageModel {
    fn default() -> Self {
        Self::ConstantPower { v_nom: Vec::new() }
    }
}

impl DistLoadVoltageModel {
    #[must_use]
    pub fn v_nom(&self) -> &[f64] {
        match self {
            Self::ConstantPower { v_nom }
            | Self::ConstantCurrent { v_nom }
            | Self::ConstantImpedance { v_nom }
            | Self::Zip { v_nom, .. }
            | Self::Exponential { v_nom, .. } => v_nom,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistGenerator {
    pub name: String,
    pub bus: String,
    pub terminal_map: Vec<String>,
    pub configuration: Configuration,
    /// Setpoint, watts per phase.
    pub p_nom: Vec<f64>,
    pub q_nom: Vec<f64>,
    pub p_min: Option<Vec<f64>>,
    pub p_max: Option<Vec<f64>>,
    pub q_min: Option<Vec<f64>>,
    pub q_max: Option<Vec<f64>>,
    /// $/kWh; no OpenDSS equivalent, so it is None until a format supplies it.
    pub cost: Option<f64>,
    pub extras: Extras,
}

impl DistGenerator {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        bus: impl Into<String>,
        terminal_map: Vec<String>,
        configuration: Configuration,
        p_nom: Vec<f64>,
        q_nom: Vec<f64>,
    ) -> Self {
        Self {
            name: name.into(),
            bus: bus.into(),
            terminal_map,
            configuration,
            p_nom,
            q_nom,
            p_min: None,
            p_max: None,
            q_min: None,
            q_max: None,
            cost: None,
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistShunt {
    pub name: String,
    pub bus: String,
    pub terminal_map: Vec<String>,
    /// Total siemens in conductor order.
    pub g: Mat,
    pub b: Mat,
    pub extras: Extras,
}

impl DistShunt {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        bus: impl Into<String>,
        terminal_map: Vec<String>,
        g: Mat,
        b: Mat,
    ) -> Self {
        Self {
            name: name.into(),
            bus: bus.into(),
            terminal_map,
            g,
            b,
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WindingConn {
    Wye,
    Delta,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Winding {
    pub bus: String,
    pub terminal_map: Vec<String>,
    pub conn: WindingConn,
    /// Rated winding voltage, volts (line to line for 2 and 3 phase).
    pub v_ref: f64,
    /// Volt amperes.
    pub s_rating: f64,
    /// Winding resistance, percent of the winding base.
    pub r_pct: f64,
    pub tap: f64,
}

impl Winding {
    #[must_use]
    pub fn new(
        bus: impl Into<String>,
        terminal_map: Vec<String>,
        conn: WindingConn,
        v_ref: f64,
        s_rating: f64,
    ) -> Self {
        Self {
            bus: bus.into(),
            terminal_map,
            conn,
            v_ref,
            s_rating,
            r_pct: 0.0,
            tap: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistTransformer {
    pub name: String,
    pub windings: Vec<Winding>,
    /// Short circuit reactances between winding pairs, percent:
    /// `[xhl]` for two windings, `[xhl, xht, xlt]` for three.
    pub xsc_pct: Vec<f64>,
    pub phases: usize,
    pub extras: Extras,
}

impl DistTransformer {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        windings: Vec<Winding>,
        xsc_pct: Vec<f64>,
        phases: usize,
    ) -> Self {
        Self {
            name: name.into(),
            windings,
            xsc_pct,
            phases,
            extras: Extras::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VoltageSource {
    pub name: String,
    pub bus: String,
    pub terminal_map: Vec<String>,
    /// Volts per terminal (0.0 on grounded terminals).
    pub v_magnitude: Vec<f64>,
    /// Radians per terminal.
    pub v_angle: Vec<f64>,
    pub extras: Extras,
}

impl VoltageSource {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        bus: impl Into<String>,
        terminal_map: Vec<String>,
        v_magnitude: Vec<f64>,
        v_angle: Vec<f64>,
    ) -> Self {
        Self {
            name: name.into(),
            bus: bus.into(),
            terminal_map,
            v_magnitude,
            v_angle,
            extras: Extras::new(),
        }
    }
}

/// An object the reader recognized but does not type: preserved by class,
/// name, and raw property text so conversions can warn precisely.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UntypedObject {
    pub class: String,
    pub name: String,
    pub props: Vec<(Option<String>, String)>,
}

impl UntypedObject {
    #[must_use]
    pub fn new(
        class: impl Into<String>,
        name: impl Into<String>,
        props: Vec<(Option<String>, String)>,
    ) -> Self {
        Self {
            class: class.into(),
            name: name.into(),
            props,
        }
    }
}

/// A multiconductor distribution network.
///
/// `source` retains the original text for the byte exact echo tier;
/// `defaulted` records, per element (`"class.name"` key), the fields the
/// reader materialized from format defaults rather than the source text.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistNetwork {
    pub name: Option<String>,
    /// Hz.
    pub base_frequency: f64,
    pub buses: Vec<DistBus>,
    pub linecodes: Vec<DistLineCode>,
    pub lines: Vec<DistLine>,
    pub switches: Vec<DistSwitch>,
    pub transformers: Vec<DistTransformer>,
    pub loads: Vec<DistLoad>,
    pub generators: Vec<DistGenerator>,
    pub shunts: Vec<DistShunt>,
    /// BMOPF allows exactly one; the model allows any number and the BMOPF
    /// writer warns beyond the first.
    pub sources: Vec<VoltageSource>,
    pub untyped: Vec<UntypedObject>,
    /// Source commands and options the typed model does not interpret
    /// (`solve`, `set mode=...`), in order, as (verb, args).
    pub commands: Vec<(String, String)>,
    pub options: Vec<(String, String)>,
    /// Per-element record of which fields were materialized from a format
    /// default. Skipped in the `.pio.json` payload: the field holds
    /// `&'static str` (no `Deserialize`), and this provenance belongs in the
    /// compiler package's `source_maps` as `mapping_kind = defaulted`, not in
    /// the raw IR payload. See
    /// <https://eigenergy.github.io/powerio/guide/pio-json-schema.html>.
    #[serde(skip)]
    pub defaulted: BTreeMap<String, Vec<&'static str>>,
    pub warnings: Vec<String>,
    /// Retained source text for the byte-exact echo tier. Skipped in the
    /// `.pio.json` payload (mirrors `powerio::Network::source`): keeping it out
    /// avoids serde's `rc` feature, and retained source is an envelope concern
    /// surfaced through `Origin::File { retained_source, .. }`.
    #[serde(skip)]
    pub source: Option<Arc<String>>,
    pub source_format: Option<DistSourceFormat>,
    pub extras: Extras,
}

/// v1-facing name for the canonical multiconductor distribution model.
pub type MulticonductorNetwork = DistNetwork;

impl Default for DistNetwork {
    /// An empty network at the OpenDSS default frequency. A derived 0 Hz
    /// default would put NaN into every capacitance the dss writer converts
    /// through omega.
    fn default() -> Self {
        DistNetwork {
            name: None,
            base_frequency: crate::dss::defaults::BASE_FREQUENCY,
            buses: Vec::new(),
            linecodes: Vec::new(),
            lines: Vec::new(),
            switches: Vec::new(),
            transformers: Vec::new(),
            loads: Vec::new(),
            generators: Vec::new(),
            shunts: Vec::new(),
            sources: Vec::new(),
            untyped: Vec::new(),
            commands: Vec::new(),
            options: Vec::new(),
            defaulted: BTreeMap::new(),
            warnings: Vec::new(),
            source: None,
            source_format: None,
            extras: Extras::new(),
        }
    }
}

impl DistNetwork {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }

    /// Case insensitive, matching the source formats' name semantics.
    pub fn bus(&self, id: &str) -> Option<&DistBus> {
        self.buses.iter().find(|b| b.id.eq_ignore_ascii_case(id))
    }

    /// Case insensitive, matching the source formats' name semantics.
    pub fn linecode(&self, name: &str) -> Option<&DistLineCode> {
        self.linecodes
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

fn zero_mat(n: usize) -> Mat {
    vec![vec![0.0; n]; n]
}

/// Builds an `n`x`n` matrix from lower triangle rows (the OpenDSS matrix
/// entry convention) or full rows; symmetric completion for the triangle.
pub(crate) fn square_from_rows(rows: &[Vec<f64>], n: usize) -> Option<Mat> {
    let mut m = vec![vec![0.0; n]; n];
    if rows.len() != n {
        return None;
    }
    let lower = rows.iter().enumerate().all(|(i, r)| r.len() == i + 1);
    let full = rows.iter().all(|r| r.len() == n);
    if lower {
        for (i, row) in rows.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                m[i][j] = v;
                m[j][i] = v;
            }
        }
    } else if full {
        for (i, row) in rows.iter().enumerate() {
            m[i].clone_from_slice(&row[..n]);
        }
    } else {
        return None;
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn lower_triangle_completes_symmetrically() {
        let rows = vec![vec![1.0], vec![0.5, 2.0], vec![0.3, 0.4, 3.0]];
        let m = square_from_rows(&rows, 3).unwrap();
        assert_eq!(m[0][1], 0.5);
        assert_eq!(m[1][0], 0.5);
        assert_eq!(m[2][2], 3.0);
        assert_eq!(m[0][2], 0.3);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn full_rows_pass_through() {
        let rows = vec![vec![1.0, 9.0], vec![8.0, 2.0]];
        let m = square_from_rows(&rows, 2).unwrap();
        assert_eq!(m[0][1], 9.0);
        assert_eq!(m[1][0], 8.0);
    }

    #[test]
    fn wrong_shape_is_rejected() {
        assert!(square_from_rows(&[vec![1.0], vec![2.0]], 2).is_none());
        assert!(square_from_rows(&[vec![1.0, 2.0]], 2).is_none());
    }
}
