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

use crate::matrix::YbusParts;
use crate::{IndexCore, IndexedNetwork, SparseMatrix};
use powerio_core::Error;
use powerio_tx::BusId;

use powerio_prob::diagnostics::codes;
use powerio_prob::{AcPfInstance, BalancedOperatingPointQuantity, OperatingPoint};

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
    /// The merged admittance rows the values derive from, built once:
    /// sorted `(column, g, b)` per bus row.
    admittance_rows: Vec<Vec<(usize, f64, f64)>>,
    /// The instance network's derived index, cached so
    /// [`update`](Self::update) rebinds a view over a fresh instance with
    /// [`IndexedNetwork::with_core`] instead of re-deriving it on every
    /// refresh. `None` for a Jacobian assembled
    /// [`from_admittance`](Self::from_admittance), which has no instance.
    index_core: Option<IndexCore>,
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
        let Some(index_core) = &self.index_core else {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
                "a Jacobian assembled from an admittance matrix has no instance to pair an \
                 operating point with; refresh it with update_from_voltages",
            ));
        };
        // `with_core` rebinds a view over the cached derivation with no
        // allocation when the instance's network has no 3-winding
        // transformer (the common case); it only re-derives the core when
        // one is present, matching `new`'s behavior exactly either way.
        let view = IndexedNetwork::with_core(instance.network(), index_core);
        // The refresh operates on the axis the structure was assembled over;
        // an instance lowering to a different bus count is a different
        // problem, never a value update.
        if view.n() != self.bus_ids.len() {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
                "the instance's lowered bus axis does not match the assembled Jacobian's",
            ));
        }
        // Identity pairing between an instance and a point is validated
        // with a real error the one time `calc_power_flow_jacobian` built
        // this Jacobian; every refresh after that trusts the same pairing
        // and only asserts it in debug builds.
        let voltages = point_voltages(instance, point, &self.bus_ids, &view, false)?;
        let injection = calc_injection(&self.admittance_rows, &voltages.magnitude, &voltages.angle);
        fill_values(
            &mut self.matrix,
            &self.admittance_rows,
            &injection,
            self.coordinates,
        );
        Ok(())
    }

    /// Assemble the Jacobian over a caller's bus admittance matrix at the
    /// voltages `vm` (per unit) and `va` (radians), in the matrix's own bus
    /// order. `bus_ids` names the bus behind each row; a preparation passes
    /// its `bus_ids` with the matrix from its `calc_admittance_matrix`. The
    /// values are the same functions of `(G, B, vm, va)` that
    /// [`calc_power_flow_jacobian`] fills, so a solver that already holds
    /// its admittance matrix and voltages builds the structure once here and
    /// refreshes it with [`update_from_voltages`](Self::update_from_voltages)
    /// on every iteration, with no operating point or identity check.
    ///
    /// # Errors
    /// An admittance matrix that is not square, a conductance and
    /// susceptance of different shapes, or `bus_ids`, `vm` or `va` whose
    /// length is not the matrix side.
    pub fn from_admittance(
        admittance: &YbusParts,
        bus_ids: Vec<BusId>,
        coordinates: VoltageCoordinates,
        vm: &[f64],
        va: &[f64],
    ) -> Result<Self, Error> {
        let n = admittance.g.rows();
        if admittance.g.cols() != n || admittance.b.shape() != (n, n) {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                "the admittance matrix the Jacobian derives from must be square, with \
                 conductance and susceptance of one shape",
            ));
        }
        if bus_ids.len() != n {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                format!(
                    "{} bus ids for an admittance matrix of side {n}",
                    bus_ids.len()
                ),
            ));
        }
        check_voltage_lengths(n, vm, va)?;
        let mut jacobian = assemble(&admittance.g, &admittance.b, bus_ids, coordinates, None);
        let injection = calc_injection(&jacobian.admittance_rows, vm, va);
        fill_values(
            &mut jacobian.matrix,
            &jacobian.admittance_rows,
            &injection,
            coordinates,
        );
        Ok(jacobian)
    }

    /// The bus power injection `S = diag(V) conj(Y V)` over this Jacobian's
    /// admittance at the voltages `vm` (per unit) and `va` (radians) in bus
    /// row order: the power flow mismatch's network term. Pass the result to
    /// [`update_from_injection`](Self::update_from_injection) to refresh the
    /// Jacobian at the same voltages without forming the injection again, so
    /// a Newton step computes it once for the mismatch and the Jacobian.
    ///
    /// # Errors
    /// `vm` or `va` whose length is not the bus count.
    pub fn calc_bus_power_injection(
        &self,
        vm: &[f64],
        va: &[f64],
    ) -> Result<BusPowerInjection, Error> {
        check_voltage_lengths(self.bus_ids.len(), vm, va)?;
        Ok(calc_injection(&self.admittance_rows, vm, va))
    }

    /// Refresh the numerical values at the voltages an injection from
    /// [`calc_bus_power_injection`](Self::calc_bus_power_injection) was
    /// formed at, reusing its currents and powers. The injection must come
    /// from this Jacobian (or one assembled over the same admittance); the
    /// values are those [`update_from_voltages`](Self::update_from_voltages)
    /// fills at the same voltages.
    ///
    /// # Errors
    /// An injection whose bus count is not this Jacobian's.
    pub fn update_from_injection(&mut self, injection: &BusPowerInjection) -> Result<(), Error> {
        if injection.p.len() != self.bus_ids.len() {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                format!(
                    "an injection over {} buses for a Jacobian over {}",
                    injection.p.len(),
                    self.bus_ids.len()
                ),
            ));
        }
        fill_values(
            &mut self.matrix,
            &self.admittance_rows,
            injection,
            self.coordinates,
        );
        Ok(())
    }

    /// Refresh the numerical values at the voltages `vm` (per unit) and `va`
    /// (radians) in bus row order. The sparse structure and admittance are
    /// reused; this is the per iteration refresh of a Newton solve.
    ///
    /// # Errors
    /// `vm` or `va` whose length is not the bus count.
    pub fn update_from_voltages(&mut self, vm: &[f64], va: &[f64]) -> Result<(), Error> {
        let injection = self.calc_bus_power_injection(vm, va)?;
        self.update_from_injection(&injection)
    }
}

/// The complex power injection `S = diag(V) conj(Y V)` at every bus, split
/// into active and reactive parts `(p, q)`, at the voltages `vm` (per unit)
/// and `va` (radians) in the admittance matrix's bus order. The injections
/// are the ones the Jacobian of [`PowerFlowJacobian::from_admittance`]
/// differentiates, formed by the same calculation as
/// [`PowerFlowJacobian::calc_bus_power_injection`]; a solver that holds a
/// Jacobian calls that method instead, which reuses its merged admittance.
///
/// # Errors
/// An admittance matrix that is not square, or voltages whose length is not
/// its side.
pub fn calc_bus_power_injection(
    admittance: &YbusParts,
    vm: &[f64],
    va: &[f64],
) -> Result<(Vec<f64>, Vec<f64>), Error> {
    let n = admittance.g.rows();
    if admittance.g.cols() != n || admittance.b.shape() != (n, n) {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            "the admittance matrix must be square, with conductance and susceptance of one shape",
        ));
    }
    check_voltage_lengths(n, vm, va)?;
    let injection = calc_injection(&merge_admittance_rows(&admittance.g, &admittance.b), vm, va);
    Ok((injection.p, injection.q))
}

fn check_voltage_lengths(n: usize, vm: &[f64], va: &[f64]) -> Result<(), Error> {
    if vm.len() != n || va.len() != n {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            format!(
                "{} voltage magnitudes and {} angles for {n} buses",
                vm.len(),
                va.len()
            ),
        ));
    }
    Ok(())
}

/// The sparse structure over an admittance pattern: each admittance entry
/// contributes to all four blocks, and every diagonal position is present for
/// the current terms. Values are left for [`fill_values`].
///
/// Row `k` and row `n + k` both hold the columns of merged admittance row
/// `k` (which always includes the diagonal), first in the angle block and
/// then in the magnitude block. The CSR is written in that layout directly,
/// so the fill walks each row beside its admittance row with no search.
fn assemble(
    conductance: &SparseMatrix,
    susceptance: &SparseMatrix,
    bus_ids: Vec<BusId>,
    coordinates: VoltageCoordinates,
    index_core: Option<IndexCore>,
) -> PowerFlowJacobian {
    let n = bus_ids.len();
    let admittance_rows = merge_admittance_rows(conductance, susceptance);
    let nnz: usize = admittance_rows.iter().map(Vec::len).sum::<usize>() * 4;
    let mut indptr = Vec::with_capacity(2 * n + 1);
    let mut indices = Vec::with_capacity(nnz);
    indptr.push(0);
    for _block in 0..2 {
        for row in &admittance_rows {
            indices.extend(row.iter().map(|&(column, _, _)| column));
            indices.extend(row.iter().map(|&(column, _, _)| n + column));
            indptr.push(indices.len());
        }
    }
    let data = vec![0.0; indices.len()];
    PowerFlowJacobian {
        coordinates,
        bus_ids,
        matrix: SparseMatrix::new((2 * n, 2 * n), indptr, indices, data),
        admittance_rows,
        index_core,
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

    let mut jacobian = assemble(&parts.g, &parts.b, bus_ids, coordinates, Some(index_core));
    let injection = calc_injection(
        &jacobian.admittance_rows,
        &voltages.magnitude,
        &voltages.angle,
    );
    fill_values(
        &mut jacobian.matrix,
        &jacobian.admittance_rows,
        &injection,
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
    // `BalancedOperatingPointBuilder` lays the columns out), so this reads
    // the typed quantity iterators rather than allocating a decimal spelling
    // of every bus id to look one up at a time.
    let raw_buses = instance.network().buses();
    let read_bus_values = |values: powerio_prob::OperatingPointValues<'_>| {
        if values.len() != raw_buses.len() {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
                "the operating point's network does not share the instance's bus identities",
            ));
        }
        let identities_match = values
            .clone()
            .zip(raw_buses)
            .all(|((id, _), bus)| id.parse::<usize>().is_ok_and(|id| id == bus.id.0));
        if validate_identities && !identities_match {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
                "the operating point's network does not share the instance's bus identities",
            ));
        } else if !validate_identities {
            debug_assert!(
                identities_match,
                "operating point bus identities do not match the instance's; this pairing \
                 is checked once when the Jacobian is built and trusted on every refresh"
            );
        }
        Ok(values.map(|(_, value)| value).collect::<Vec<_>>())
    };

    // Both quantities' complete columns, one bulk read each rather than one
    // string keyed lookup per bus per quantity.
    let (Some(magnitude_values), Some(angle_values)) = (
        point.values(BalancedOperatingPointQuantity::BusVoltageMagnitude),
        point.values(BalancedOperatingPointQuantity::BusVoltageAngle),
    ) else {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            "the operating point does not state a complete complex voltage at every bus; \
             the Jacobian needs both quantities",
        ));
    };

    let raw_n = raw_buses.len();
    let mut magnitude = read_bus_values(magnitude_values)?;
    let mut angle = read_bus_values(angle_values)?;
    magnitude.reserve(bus_ids.len() - raw_n);
    angle.reserve(bus_ids.len() - raw_n);
    for (idx, &bus) in bus_ids.iter().enumerate().skip(raw_n) {
        // A star bus the three winding expansion synthesized: the point
        // cannot state it, so its voltage comes from the lowered network's
        // own stated values, through a checked lookup.
        let star = view
            .network()
            .buses()
            .get(idx)
            .filter(|star| star.id == bus);
        let Some(star) = star else {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
                "the operating point's network does not share the instance's bus identities",
            ));
        };
        magnitude.push(star.vm);
        angle.push(view.to_radians(star.va));
    }
    Ok(Voltages { magnitude, angle })
}

/// Fill every value of the assembled structure at the voltages `injection`
/// was formed at. The fill walks each Jacobian row beside its merged
/// admittance row (the layout [`assemble`] writes), so the cost follows the
/// nonzero count, never the dense square, and reuses the injection's one
/// sine and cosine per bus.
#[allow(clippy::many_single_char_names)] // k/m/n and G/B are the textbook notation
fn fill_values(
    matrix: &mut SparseMatrix,
    admittance_rows: &[Vec<(usize, f64, f64)>],
    injection: &BusPowerInjection,
    coordinates: VoltageCoordinates,
) {
    let BusPowerInjection {
        p,
        q,
        vm,
        cos,
        sin,
        current_re,
        current_im,
    } = injection;
    let n = vm.len();
    let half = matrix.nnz() / 2;
    let data = matrix.data_mut();
    let (active, reactive) = data.split_at_mut(half);
    let mut offset = 0;
    for (k, row) in admittance_rows.iter().enumerate() {
        let len = row.len();
        let (p_angle, p_magnitude) = active[offset..offset + 2 * len].split_at_mut(len);
        let (q_angle, q_magnitude) = reactive[offset..offset + 2 * len].split_at_mut(len);
        offset += 2 * len;
        for (position, &(m, g, b)) in row.iter().enumerate() {
            let entries = match coordinates {
                VoltageCoordinates::Polar => {
                    if k == m {
                        // MATPOWER dSbus_dV diagonals.
                        let vk2 = vm[k] * vm[k];
                        let (p_over, q_over) = if vm[k] == 0.0 {
                            (0.0, 0.0)
                        } else {
                            (p[k] / vm[k], q[k] / vm[k])
                        };
                        [
                            -q[k] - b * vk2,
                            p_over + g * vm[k],
                            p[k] - g * vk2,
                            q_over - b * vm[k],
                        ]
                    } else {
                        // sin and cos of va[k] - va[m] from the per bus values.
                        let cos_km = cos[k] * cos[m] + sin[k] * sin[m];
                        let sin_km = sin[k] * cos[m] - cos[k] * sin[m];
                        let odd = g * sin_km - b * cos_km;
                        let even = g * cos_km + b * sin_km;
                        let vkm = vm[k] * vm[m];
                        [vkm * odd, vm[k] * even, -vkm * even, vm[k] * odd]
                    }
                }
                VoltageCoordinates::Cartesian => {
                    // dS_k/dVr_m = δ conj(I_k) + V_k conj(Y_km);
                    // dS_k/dVi_m = jδ conj(I_k) − j V_k conj(Y_km).
                    let vr_k = vm[k] * cos[k];
                    let vi_k = vm[k] * sin[k];
                    let real_part = vr_k * g + vi_k * b;
                    let imag_part = vi_k * g - vr_k * b;
                    if k == m {
                        [
                            real_part + current_re[k],
                            imag_part + current_im[k],
                            imag_part - current_im[k],
                            -real_part + current_re[k],
                        ]
                    } else {
                        [real_part, imag_part, imag_part, -real_part]
                    }
                }
            };
            // [(P, angle), (P, magnitude), (Q, angle), (Q, magnitude)].
            p_angle[position] = entries[0];
            p_magnitude[position] = entries[1];
            q_angle[position] = entries[2];
            q_magnitude[position] = entries[3];
        }
    }
    debug_assert_eq!(offset, half);
    debug_assert_eq!(n, admittance_rows.len());
}

/// Merged sparse admittance rows: sorted `(column, g, b)` per bus row, with
/// the diagonal always present (zero where neither matrix stores it).
fn merge_admittance_rows(
    conductance: &SparseMatrix,
    susceptance: &SparseMatrix,
) -> Vec<Vec<(usize, f64, f64)>> {
    let n = conductance.rows();
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
    for (k, row) in admittance_rows.iter_mut().enumerate() {
        if let Err(position) = row.binary_search_by_key(&k, |entry| entry.0) {
            row.insert(position, (k, 0.0, 0.0));
        }
    }
    admittance_rows
}

/// The bus power injection `S = diag(V) conj(Y V)` at one voltage point:
/// `p` and `q` in bus row order, per unit on the admittance's base. It also
/// keeps the voltages, per bus angle cosines and sines, and currents
/// `I = Y V` it was formed from, so
/// [`PowerFlowJacobian::update_from_injection`] refreshes the Jacobian at the
/// same point without recomputing them.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BusPowerInjection {
    /// Active power injection per bus.
    pub p: Vec<f64>,
    /// Reactive power injection per bus.
    pub q: Vec<f64>,
    vm: Vec<f64>,
    cos: Vec<f64>,
    sin: Vec<f64>,
    current_re: Vec<f64>,
    current_im: Vec<f64>,
}

/// The one calculation of `I = Y V` and `S = diag(V) conj(I)` over merged
/// admittance rows: every injection this module reports or differentiates
/// comes from here.
fn calc_injection(
    admittance_rows: &[Vec<(usize, f64, f64)>],
    vm: &[f64],
    va: &[f64],
) -> BusPowerInjection {
    let n = vm.len();
    let (sin, cos): (Vec<f64>, Vec<f64>) = va.iter().map(|angle| angle.sin_cos()).unzip();
    let vr: Vec<f64> = (0..n).map(|k| vm[k] * cos[k]).collect();
    let vi: Vec<f64> = (0..n).map(|k| vm[k] * sin[k]).collect();
    let mut current_re = vec![0.0; n];
    let mut current_im = vec![0.0; n];
    for (k, row) in admittance_rows.iter().enumerate() {
        let mut ir = 0.0;
        let mut ii = 0.0;
        for &(m, g, b) in row {
            ir += g * vr[m] - b * vi[m];
            ii += g * vi[m] + b * vr[m];
        }
        current_re[k] = ir;
        current_im[k] = ii;
    }
    let p: Vec<f64> = (0..n)
        .map(|k| vr[k] * current_re[k] + vi[k] * current_im[k])
        .collect();
    let q: Vec<f64> = (0..n)
        .map(|k| vi[k] * current_re[k] - vr[k] * current_im[k])
        .collect();
    BusPowerInjection {
        p,
        q,
        vm: vm.to_vec(),
        cos,
        sin,
        current_re,
        current_im,
    }
}
