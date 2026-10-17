//! Full nonreciprocal admittances as diagonally shifted two-port blocks.
use super::asymmetric_pair::{self, PairPorts};
use super::{C, DssWriter, num};
use crate::{DistLineCode, DistShunt, MulticonductorNetwork};
use num_complex::Complex64;

pub(super) fn needed(shunt: &DistShunt) -> bool {
    [&shunt.g, &shunt.b].into_iter().any(|m| {
        m.iter().enumerate().any(|(i, row)| {
            row.iter()
                .enumerate()
                .any(|(j, v)| m.get(j).and_then(|r| r.get(i)) != Some(v))
        })
    })
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
        {
            return Err(format!(
                "shunt {}: asymmetric admittance requires declared terminals and a finite exact DSS projection",
                shunt.name
            ));
        }
    }
    Ok(())
}
impl DssWriter {
    pub(super) fn asymmetric_shunt(
        &mut self,
        net: &MulticonductorNetwork,
        shunt: &DistShunt,
        index: usize,
    ) -> bool {
        if !needed(shunt) {
            return false;
        }
        let projection = projection(shunt).expect("validated asymmetric admittance");
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
        self.warn(&C::EMIT_DSS_IMPEDANCE_EQUIVALENT,format!("shunt {}: full asymmetric admittance uses {} coupled two-port blocks and exact diagonal corrections under prefix {prefix}; aggregate external terminal currents instead of interpreting internal negative-element losses",shunt.name,projection.pairs.len()));
        for key in shunt.extras.keys() {
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
