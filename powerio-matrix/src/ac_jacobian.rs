//! The sparse AC power flow Jacobian at an operating point (#407).
//!
//! [`calc_power_flow_jacobian`] returns the full physical derivative of every
//! bus active and reactive power injection with respect to every voltage
//! coordinate, `2n × 2n`, over an [`AcPfInstance`] plus a separately supplied
//! complete [`OperatingPoint`] sharing the same network identities. It does
//! not replace columns with generator injection variables, remove fixed
//! variables, or add voltage setpoint equations: those belong to the solver
//! that selects its equations and variable arrangement.
//!
//! Row `k` is bus `k`'s active power and row `n + k` its reactive power, in
//! bus table order. Under polar coordinates column `m` is bus `m`'s voltage
//! angle (radians) and column `n + m` its voltage magnitude (per unit); under
//! Cartesian coordinates the columns are the real then imaginary voltage. All
//! powers are per unit on the network MVA base.
//!
//! The sparse structure is allocated once from the admittance matrix pattern;
//! [`PowerFlowJacobian::update`] refreshes the numerical values in place
//! across operating points with unchanged topology.

use crate::{IndexCore, IndexedNetwork, SparseMatrix};
use powerio_core::Error;
use powerio_tx::BusId;

use powerio_prob::AcPfInstance;
use powerio_prob::OperatingPoint;
use powerio_prob::diagnostics::codes;

/// The balanced instantaneous quantity keys behind
/// `OperatingPoint::bus_voltage_magnitude`/`bus_voltage_angle`
/// (`powerio-prob/src/state/balanced.rs`). Not exported from that crate as
/// named constants, so duplicated here as the literal spellings the
/// `.pio.json` wire form (and every `powerio-prob` writer) already treats as
/// permanent.
const BUS_VOLTAGE_MAGNITUDE: &str = "bus_voltage_magnitude";
const BUS_VOLTAGE_ANGLE: &str = "bus_voltage_angle";

/// The voltage coordinate selection, the one option of the calculation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum VoltageCoordinates {
    /// Angle (radians) then magnitude (per unit) columns.
    #[default]
    Polar,
    /// Real then imaginary voltage columns, both per unit.
    Cartesian,
}

/// The assembled sparse physical Jacobian, with its bus mappings and the
/// machinery to update values in place.
#[derive(Clone, Debug)]
pub struct PowerFlowJacobian {
    coordinates: VoltageCoordinates,
    bus_ids: Vec<BusId>,
    matrix: SparseMatrix,
    /// The admittance parts the values derive from, built once.
    conductance: SparseMatrix,
    susceptance: SparseMatrix,
    /// The instance network's derived index, cached so
    /// [`update`](Self::update) rebinds a view over a fresh instance with
    /// [`IndexedNetwork::with_core`] instead of re-deriving it on every
    /// refresh.
    index_core: IndexCore,
}

impl PowerFlowJacobian {
    /// The `2n × 2n` sparse matrix. Row `k` is active power at bus `k`, row
    /// `n + k` reactive power; the columns follow
    /// [`coordinates`](Self::coordinates).
    #[must_use]
    pub const fn matrix(&self) -> &SparseMatrix {
        &self.matrix
    }

    /// The bus behind row and column block position `k`, for both blocks of
    /// both dimensions.
    #[must_use]
    pub fn bus_ids(&self) -> &[BusId] {
        &self.bus_ids
    }

    /// The selected voltage coordinates.
    #[must_use]
    pub const fn coordinates(&self) -> VoltageCoordinates {
        self.coordinates
    }

    /// Refresh the numerical values at a new operating point. The sparse
    /// structure and the admittance parts are reused; topology changes
    /// require a rebuild through [`calc_power_flow_jacobian`].
    ///
    /// # Errors
    /// An operating point that does not share the instance's network
    /// identities or does not state both voltage quantities.
    pub fn update(
        &mut self,
        instance: &AcPfInstance,
        point: &OperatingPoint<powerio_tx::BalancedNetwork>,
    ) -> Result<(), Error> {
        // `with_core` rebinds a view over the cached derivation with no
        // allocation when the instance's network has no 3-winding
        // transformer (the common case); it only re-derives the core when
        // one is present, matching `new`'s behavior exactly either way.
        let view = IndexedNetwork::with_core(instance.network(), &self.index_core);
        // The refresh operates on the axis the structure was assembled over;
        // an instance lowering to a different bus count is a different
        // problem, never a value update.
        if view.n() != self.bus_ids.len() {
            return Err(Error::new(
                &codes::BUILD_STATE_IDENTITY_UNKNOWN,
                "the instance's lowered bus axis does not match the assembled Jacobian's",
            ));
        }
        // Identity pairing between an instance and a point is validated
        // with a real error the one time `calc_power_flow_jacobian` built
        // this Jacobian; every refresh after that trusts the same pairing
        // and only asserts it in debug builds.
        let voltages = point_voltages(instance, point, &self.bus_ids, &view, false)?;
        fill_values(
            &mut self.matrix,
            &self.conductance,
            &self.susceptance,
            &voltages,
            self.coordinates,
        );
        Ok(())
    }
}

/// One complete complex voltage assignment, split for the fill.
struct Voltages {
    magnitude: Vec<f64>,
    angle: Vec<f64>,
}

/// Compute the sparse physical AC power flow Jacobian for `instance` at
/// `point`.
///
/// # Errors
/// An operating point that does not share the instance's network identities
/// or does not state bus voltage magnitudes and angles; an admittance build
/// failure (a zero impedance branch is refused, never skipped).
pub fn calc_power_flow_jacobian(
    instance: &AcPfInstance,
    point: &OperatingPoint<powerio_tx::BalancedNetwork>,
    coordinates: VoltageCoordinates,
) -> Result<PowerFlowJacobian, Error> {
    let network = instance.network();
    // Derived once here and cached on the returned Jacobian, so
    // `update` rebinds a view over it with `IndexedNetwork::with_core`
    // instead of re-deriving it on every refresh.
    let index_core = IndexCore::build(network);
    // The analysis axis comes from the same lowered view the admittance is
    // built from, so a three winding expansion's star buses are part of the
    // pattern and the assembled dimension always equals the admittance
    // dimension.
    let view = IndexedNetwork::with_core(network, &index_core);
    let bus_ids: Vec<BusId> = (0..view.n()).map(|idx| view.bus_id(idx)).collect();
    // The one point this pairing is validated with a real error; every
    // later `update` call trusts it instead (see `point_voltages`).
    let voltages = point_voltages(instance, point, &bus_ids, &view, true)?;

    let parts = crate::calc_admittance_matrix(
        &view,
        &crate::BuildOptions {
            skip_zero_impedance: false,
            ..Default::default()
        },
    )
    .map_err(|error| {
        Error::new(
            &codes::BUILD_OPERATOR_ZERO_IMPEDANCE,
            format!(
                "the admittance matrix the Jacobian derives from cannot be built: {error}; resolve zero impedance branches explicitly with merge_zero_impedance_buses"
            ),
        )
    })?;

    let n = bus_ids.len();
    // The structure comes from the admittance pattern once: each admittance
    // entry contributes to all four blocks, and every diagonal position is
    // present for the current terms.
    let mut pattern = crate::matrix::triplet::CooBuilder::new(2 * n);
    for (row, row_vec) in parts.g.outer_iterator().enumerate() {
        for (column, _) in row_vec.iter() {
            pattern.add(row, column, 1.0);
            pattern.add(row, n + column, 1.0);
            pattern.add(n + row, column, 1.0);
            pattern.add(n + row, n + column, 1.0);
        }
    }
    for (row, row_vec) in parts.b.outer_iterator().enumerate() {
        for (column, _) in row_vec.iter() {
            pattern.add(row, column, 1.0);
            pattern.add(row, n + column, 1.0);
            pattern.add(n + row, column, 1.0);
            pattern.add(n + row, n + column, 1.0);
        }
    }
    for row in 0..n {
        pattern.add(row, row, 1.0);
        pattern.add(row, n + row, 1.0);
        pattern.add(n + row, row, 1.0);
        pattern.add(n + row, n + row, 1.0);
    }

    let mut jacobian = PowerFlowJacobian {
        coordinates,
        bus_ids,
        matrix: pattern.finish_csr(),
        conductance: parts.g,
        susceptance: parts.b,
        index_core,
    };
    fill_values(
        &mut jacobian.matrix,
        &jacobian.conductance,
        &jacobian.susceptance,
        &voltages,
        coordinates,
    );
    Ok(jacobian)
}

/// The complete voltage assignment from the operating point, in bus row
/// order, with the identity checks #407 requires.
///
/// `validate_identities` runs the network pairing check with a real error on
/// mismatch — [`calc_power_flow_jacobian`] passes `true` the one time it
/// builds a Jacobian. [`PowerFlowJacobian::update`] passes `false`: every
/// refresh reuses the pairing that build already proved, so re-checking it
/// in full on each call would cost as much as the rebuild `update` exists to
/// avoid; the same check still runs as a `debug_assert` to catch a
/// genuine misuse in tests.
fn point_voltages(
    instance: &AcPfInstance,
    point: &OperatingPoint<powerio_tx::BalancedNetwork>,
    bus_ids: &[BusId],
    view: &IndexedNetwork<'_>,
    validate_identities: bool,
) -> Result<Voltages, Error> {
    // The point must share the instance's own bus identity list exactly, in
    // the same order; the analysis axis may extend it with the expansion's
    // star buses, whose voltages come from the lowered network below. Every
    // bus quantity's column order is `network.buses()` order (how
    // `BalancedStateBuilder` lays the columns out), so this reads straight
    // off `identity_order` rather than allocating a decimal spelling of
    // every bus id to look one up at a time.
    let raw_buses = instance.network().buses();
    let identities_match = |quantity: &str| -> bool {
        point.identity_order(quantity).is_some_and(|order| {
            order.len() == raw_buses.len()
                && order
                    .zip(raw_buses)
                    .all(|(id, bus)| id.parse::<usize>().is_ok_and(|id| id == bus.id.0))
        })
    };
    if validate_identities {
        if !identities_match(BUS_VOLTAGE_MAGNITUDE) {
            return Err(Error::new(
                &codes::BUILD_STATE_IDENTITY_UNKNOWN,
                "the operating point's network does not share the instance's bus identities",
            ));
        }
    } else {
        debug_assert!(
            identities_match(BUS_VOLTAGE_MAGNITUDE),
            "operating point bus identities do not match the instance's; this pairing \
             is checked once when the Jacobian is built and trusted on every refresh"
        );
    }

    // Both quantities' complete columns, one bulk read each rather than one
    // string keyed lookup per bus per quantity.
    let (Some(magnitude_column), Some(angle_column)) = (
        point.quantity_values(BUS_VOLTAGE_MAGNITUDE),
        point.quantity_values(BUS_VOLTAGE_ANGLE),
    ) else {
        return Err(Error::new(
            &codes::BUILD_STATE_SHAPE_MISMATCH,
            "the operating point does not state a complete complex voltage at every bus; \
             the Jacobian needs both quantities",
        ));
    };

    let raw_n = raw_buses.len();
    let mut magnitude = Vec::with_capacity(bus_ids.len());
    let mut angle = Vec::with_capacity(bus_ids.len());
    for (idx, &bus) in bus_ids.iter().enumerate() {
        if idx < raw_n {
            magnitude.push(magnitude_column[idx]);
            angle.push(angle_column[idx]);
        } else {
            // A star bus the three winding expansion synthesized: the point
            // cannot state it, so its voltage comes from the lowered
            // network's own stated values, through a checked lookup.
            let star = view
                .network()
                .buses()
                .get(idx)
                .filter(|star| star.id == bus);
            let Some(star) = star else {
                return Err(Error::new(
                    &codes::BUILD_STATE_IDENTITY_UNKNOWN,
                    "the operating point's network does not share the instance's bus identities",
                ));
            };
            magnitude.push(star.vm);
            angle.push(view.to_radians(star.va));
        }
    }
    Ok(Voltages { magnitude, angle })
}

/// Fill every value of the assembled structure at the given voltages. The
/// fill walks the fixed sparse structure once: each entry's value comes from
/// its `(bus row, bus column, block)` position and the merged admittance row,
/// so the cost follows the nonzero count, never the dense square.
#[allow(clippy::many_single_char_names)] // k/m/n and G/B are the textbook notation
#[allow(clippy::too_many_lines)] // the four blocks of both coordinate systems, stated in full
#[allow(clippy::match_same_arms)] // block positions coincide numerically, never semantically
fn fill_values(
    matrix: &mut SparseMatrix,
    conductance: &SparseMatrix,
    susceptance: &SparseMatrix,
    voltages: &Voltages,
    coordinates: VoltageCoordinates,
) {
    let n = voltages.magnitude.len();
    let vm = &voltages.magnitude;
    let va = &voltages.angle;

    // Merged sparse admittance rows: sorted `(m, g, b)` per bus row.
    let mut admittance_rows: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
    for (row, row_vec) in conductance.outer_iterator().enumerate() {
        for (column, &g) in row_vec.iter() {
            admittance_rows[row].push((column, g, 0.0));
        }
    }
    for (row, row_vec) in susceptance.outer_iterator().enumerate() {
        for (column, &b) in row_vec.iter() {
            match admittance_rows[row].binary_search_by_key(&column, |entry| entry.0) {
                Ok(position) => admittance_rows[row][position].2 = b,
                Err(position) => admittance_rows[row].insert(position, (column, 0.0, b)),
            }
        }
    }
    let admittance_at = |k: usize, m: usize| -> (f64, f64) {
        match admittance_rows[k].binary_search_by_key(&m, |entry| entry.0) {
            Ok(position) => {
                let (_, g, b) = admittance_rows[k][position];
                (g, b)
            }
            Err(_) => (0.0, 0.0),
        }
    };

    // Current injection I = Y V and complex power S = diag(V) conj(I), for
    // the diagonal terms.
    let mut current_re = vec![0.0; n];
    let mut current_im = vec![0.0; n];
    for (k, row) in admittance_rows.iter().enumerate() {
        let mut ir = 0.0;
        let mut ii = 0.0;
        for &(m, g, b) in row {
            let (sin, cos) = va[m].sin_cos();
            let vr = vm[m] * cos;
            let vi = vm[m] * sin;
            ir += g * vr - b * vi;
            ii += g * vi + b * vr;
        }
        current_re[k] = ir;
        current_im[k] = ii;
    }
    let p: Vec<f64> = (0..n)
        .map(|k| vm[k] * va[k].cos() * current_re[k] + vm[k] * va[k].sin() * current_im[k])
        .collect();
    let q: Vec<f64> = (0..n)
        .map(|k| vm[k] * va[k].sin() * current_re[k] - vm[k] * va[k].cos() * current_im[k])
        .collect();

    for (row, mut row_vec) in matrix.outer_iterator_mut().enumerate() {
        let k = row % n;
        let reactive_row = row >= n;
        for (column, value) in row_vec.iter_mut() {
            let m = column % n;
            let magnitude_column = column >= n;
            let (g, b) = admittance_at(k, m);
            *value = match coordinates {
                VoltageCoordinates::Polar => {
                    if k == m {
                        // MATPOWER dSbus_dV diagonals.
                        match (reactive_row, magnitude_column) {
                            (false, false) => -q[k] - b * vm[k] * vm[k],
                            (true, false) => p[k] - g * vm[k] * vm[k],
                            (false, true) => {
                                let over = if vm[k] == 0.0 { 0.0 } else { p[k] / vm[k] };
                                over + g * vm[k]
                            }
                            (true, true) => {
                                let over = if vm[k] == 0.0 { 0.0 } else { q[k] / vm[k] };
                                over - b * vm[k]
                            }
                        }
                    } else {
                        let (sin, cos) = (va[k] - va[m]).sin_cos();
                        let odd = g * sin - b * cos;
                        let even = g * cos + b * sin;
                        match (reactive_row, magnitude_column) {
                            (false, false) => vm[k] * vm[m] * odd,
                            (true, false) => -vm[k] * vm[m] * even,
                            (false, true) => vm[k] * even,
                            (true, true) => vm[k] * odd,
                        }
                    }
                }
                VoltageCoordinates::Cartesian => {
                    // dS_k/dVr_m = δ conj(I_k) + V_k conj(Y_km);
                    // dS_k/dVi_m = jδ conj(I_k) − j V_k conj(Y_km).
                    let vr_k = vm[k] * va[k].cos();
                    let vi_k = vm[k] * va[k].sin();
                    let real_part = vr_k * g + vi_k * b;
                    let imag_part = vi_k * g - vr_k * b;
                    let mut entry = match (reactive_row, magnitude_column) {
                        (false, false) => real_part,
                        (true, false) => imag_part,
                        (false, true) => imag_part,
                        (true, true) => -real_part,
                    };
                    if k == m {
                        entry += match (reactive_row, magnitude_column) {
                            (false, false) => current_re[k],
                            (true, false) => -current_im[k],
                            (false, true) => current_im[k],
                            (true, true) => current_re[k],
                        };
                    }
                    entry
                }
            };
        }
    }
}
