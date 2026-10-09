//! Full admittances: reciprocal branches or shifted nonreciprocal two-port blocks.
use super::asymmetric_pair::{self, PairPorts};
use super::{C, DssWriter, num};
use crate::{DistLineCode, DistShunt, MulticonductorNetwork};
use num_complex::Complex64;

pub(super) fn needed(shunt: &DistShunt) -> bool {
    let n = shunt.terminal_map.len();
    let phases = super::extras_usize(&shunt.extras, "phases").unwrap_or(n);
    if symmetric(shunt)
        && !super::has_nonzero(&shunt.g)
        && (super::shunt_stashed_delta(shunt) || super::looks_like_delta_shunt(&shunt.b, n, phases))
        && super::delta_branch_susceptance(&shunt.b, &super::delta_edges(n, phases), n).is_some()
    {
        return false; // Existing scalar delta expression is already exact.
    }
    // Scalar native shunts cannot preserve coupling or unequal diagonal
    // values, even when the full matrix is reciprocal.
    [&shunt.g, &shunt.b].into_iter().any(|matrix| {
        matrix.iter().enumerate().any(|(i, row)| {
            row.iter().enumerate().any(|(j, value)| {
                if i == j {
                    matrix.first().and_then(|r| r.first()) != Some(value)
                } else {
                    *value != 0.0
                }
            })
        })
    })
}
fn symmetric(shunt: &DistShunt) -> bool {
    [&shunt.g, &shunt.b].into_iter().all(|m| {
        m.iter().enumerate().all(|(i, r)| {
            r.iter()
                .enumerate()
                .all(|(j, v)| m.get(j).and_then(|r| r.get(i)) == Some(v))
        })
    })
}
struct ReciprocalProjection {
    branches: Vec<(usize, usize, Complex64)>,
    diagonal: Vec<Option<Complex64>>,
}
fn reciprocal_projection(shunt: &DistShunt) -> Option<ReciprocalProjection> {
    let n = shunt.terminal_map.len();
    let value = |i: usize, j: usize| Complex64::new(shunt.g[i][j], shunt.b[i][j]);
    let impedance = |y: Complex64| -> Option<Complex64> {
        let z = 1.0 / y;
        (z.re.is_finite() && z.im.is_finite() && z != Complex64::default()).then_some(z)
    };
    let mut diagonal = (0..n).map(|i| value(i, i)).collect::<Vec<_>>();
    let mut branches = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let off = value(i, j);
            if off != Complex64::default() {
                branches.push((i, j, impedance(-off)?));
                diagonal[i] += off;
                diagonal[j] += off;
            }
        }
    }
    let diagonal = diagonal
        .into_iter()
        .map(|y| {
            if y == Complex64::default() {
                Some(None)
            } else {
                impedance(y).map(Some)
            }
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ReciprocalProjection { branches, diagonal })
}
struct Pair {
    first: usize,
    second: usize,
    impedance: ([Complex64; 3], [Complex64; 2]),
}
struct Projection {
    pairs: Vec<Pair>,
    diagonal: Vec<Option<Complex64>>,
}
fn projection(shunt: &DistShunt) -> Option<Projection> {
    let n = shunt.terminal_map.len();
    if n == 0
        || [&shunt.g, &shunt.b].iter().any(|m| {
            m.len() != n
                || m.iter()
                    .any(|r| r.len() != n || r.iter().any(|v| !v.is_finite()))
        })
    {
        return None;
    }
    let value = |i: usize, j: usize| Complex64::new(shunt.g[i][j], shunt.b[i][j]);
    let mut diagonal = (0..n).map(|i| value(i, i)).collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let upper = value(i, j);
            let lower = value(j, i);
            if upper == Complex64::default() && lower == Complex64::default() {
                continue;
            }
            // Shift both diagonal entries to make every block safely invertible;
            // remove that exact shift from the remaining diagonal admittances.
            let shift = 2.0 * (upper.norm() + lower.norm());
            if !shift.is_finite() || shift <= 0.0 {
                return None;
            }
            let det = Complex64::new(shift * shift, 0.0) - upper * lower;
            if det == Complex64::default() || !det.re.is_finite() || !det.im.is_finite() {
                return None;
            }
            let z = [
                [Complex64::new(shift, 0.0) / det, -upper / det],
                [-lower / det, Complex64::new(shift, 0.0) / det],
            ];
            let code = DistLineCode::new(
                "pair",
                z.iter().map(|r| r.iter().map(|v| v.re).collect()).collect(),
                z.iter().map(|r| r.iter().map(|v| v.im).collect()).collect(),
            );
            pairs.push(Pair {
                first: i,
                second: j,
                impedance: asymmetric_pair::equivalent(&code, 1.0)?,
            });
            diagonal[i] -= shift;
            diagonal[j] -= shift;
        }
    }
    let diagonal = diagonal
        .into_iter()
        .map(|y| {
            if y == Complex64::default() {
                return Some(None);
            }
            let z = 1.0 / y;
            (z.re.is_finite() && z.im.is_finite() && z != Complex64::default()).then_some(Some(z))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Projection { pairs, diagonal })
}
pub(super) fn validate(net: &MulticonductorNetwork) -> Result<(), String> {
    for shunt in net.shunts().iter().filter(|s| needed(s)) {
        if !net.buses().iter().any(|b| {
            b.id.eq_ignore_ascii_case(&shunt.bus)
                && shunt.terminal_map.iter().all(|t| b.terminals.contains(t))
        }) || projection(shunt).is_none()
            || (symmetric(shunt) && reciprocal_projection(shunt).is_none())
        {
            return Err(format!(
                "shunt {}: full admittance requires declared terminals and a finite exact DSS projection",
                shunt.name
            ));
        }
    }
    Ok(())
}
impl DssWriter {
    pub(super) fn full_admittance_shunt(
        &mut self,
        net: &MulticonductorNetwork,
        shunt: &DistShunt,
        index: usize,
    ) -> bool {
        if !needed(shunt) {
            return false;
        }
        let mut projection = projection(shunt).expect("validated full admittance");
        let mut prefix = format!("_pio_shunt_{index}_");
        let mut names = super::ideal_transformers::used_names(net);
        names.extend(net.shunts().iter().map(|s| s.name.to_ascii_lowercase()));
        names.extend(net.loads().iter().map(|s| s.name.to_ascii_lowercase()));
        names.extend(net.generators().iter().map(|s| s.name.to_ascii_lowercase()));
        names.extend(net.sources().iter().map(|s| s.name.to_ascii_lowercase()));
        names.extend(net.ibrs().iter().map(|s| s.name.to_ascii_lowercase()));
        while names.iter().any(|n| n.contains(&prefix)) {
            prefix.push('_');
        }
        if symmetric(shunt) {
            let reciprocal = reciprocal_projection(shunt).expect("validated reciprocal admittance");
            for (k, (i, j, z)) in reciprocal.branches.iter().enumerate() {
                let from = self.bus_ref(&shunt.bus, std::slice::from_ref(&shunt.terminal_map[*i]));
                let to = self.bus_ref(&shunt.bus, std::slice::from_ref(&shunt.terminal_map[*j]));
                self.line_out(&format!("New Line.{prefix}pair_{k} phases=1 bus1={from} bus2={to} length=1 units=m r1={} x1={} r0={} x0={} c1=0 c0=0 basefreq={}",num(z.re),num(z.im),num(z.re),num(z.im),num(net.base_frequency())));
            }
            projection.pairs.clear();
            projection.diagonal = reciprocal.diagonal;
        }
        let ground = format!("{}.0.0", shunt.bus);
        for (k, pair) in projection.pairs.iter().enumerate() {
            let name = format!("{prefix}pair_{k}");
            let auxiliary = format!("{prefix}bus_{k}");
            let from = self.bus_ref(
                &shunt.bus,
                &[
                    shunt.terminal_map[pair.first].clone(),
                    shunt.terminal_map[pair.second].clone(),
                ],
            );
            self.coupled_pair(
                PairPorts {
                    name: &name,
                    auxiliary: &auxiliary,
                    from: &from,
                    to: &ground,
                },
                net.base_frequency(),
                pair.impedance,
            );
        }
        for (k, impedance) in projection.diagonal.iter().enumerate() {
            if let Some(z) = impedance {
                let bus = self.bus_ref(&shunt.bus, std::slice::from_ref(&shunt.terminal_map[k]));
                self.line_out(&format!("New Reactor.{prefix}diagonal_{k} phases=1 bus1={bus} bus2={}.0 r={} x={} basefreq={}",shunt.bus,num(z.re),num(z.im),num(net.base_frequency())));
            }
        }
        self.warn(&C::EMIT_DSS_IMPEDANCE_EQUIVALENT,format!("shunt {}: full admittance uses reciprocal impedance branches or {} coupled two-port blocks and exact diagonal corrections under prefix {prefix}; aggregate external terminal currents instead of interpreting internal negative-element losses",shunt.name,projection.pairs.len()));
        for key in shunt.extras.keys().filter(|key| {
            !matches!(
                key.as_str(),
                "dss_original_element" | "dss_external_contacts"
            )
        }) {
            self.warn(
                &C::EMIT_DSS_EXTRAS_DROPPED,
                format!(
                    "shunt {}: extra {key} has no field on its exact multi-element equivalent",
                    shunt.name
                ),
            );
        }
        true
    }
}
