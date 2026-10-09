//! Exact two-conductor impedance using an open third sequence-source port.
use super::{DssWriter, num, sources};
use crate::{DistLine, DistLineCode, MulticonductorNetwork};
use num_complex::Complex64;
use std::fmt::Write as _;

pub(super) fn equivalent(
    code: &DistLineCode,
    length: f64,
) -> Option<([Complex64; 3], [Complex64; 2])> {
    if code.n_conductors != 2
        || !length.is_finite()
        || length <= 0.0
        || [&code.g_from, &code.g_to, &code.b_from, &code.b_to]
            .into_iter()
            .any(|m| m.iter().flatten().any(|&v| v != 0.0))
    {
        return None;
    }
    let mut z = [[Complex64::default(); 2]; 2];
    for (i, row) in z.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = Complex64::new(
                *code.r_series.get(i)?.get(j)?,
                *code.x_series.get(i)?.get(j)?,
            ) * length;
        }
    }
    let diagonal = 2.0 * z.iter().flatten().map(|v| v.norm()).sum::<f64>();
    if !diagonal.is_finite() || diagonal <= 0.0 {
        return None;
    }
    let c = Complex64::new(diagonal, 0.0);
    let circulant = [
        [c, z[0][1], z[1][0]],
        [z[1][0], c, z[0][1]],
        [z[0][1], z[1][0], c],
    ];
    let full = DistLineCode::new(
        "pair",
        circulant
            .iter()
            .map(|r| r.iter().map(|v| v.re).collect())
            .collect(),
        circulant
            .iter()
            .map(|r| r.iter().map(|v| v.im).collect())
            .collect(),
    );
    let seq = sources::sequence_impedance(&full, 1.0)?;
    let corrections = [z[0][0] - c, z[1][1] - c];
    corrections
        .iter()
        .all(|v| v.re.is_finite() && v.im.is_finite())
        .then_some((seq, corrections))
}
impl DssWriter {
    pub(super) fn asymmetric_pair_branch(
        &mut self,
        net: &MulticonductorNetwork,
        line: &DistLine,
        code: &DistLineCode,
    ) {
        let (sequence, correction) =
            equivalent(code, line.length).expect("validated two-conductor impedance");
        let index = net
            .lines()
            .iter()
            .position(|l| std::ptr::eq(l, line))
            .expect("network line");
        let mut auxiliary = format!("_pio_pair_{index}");
        while net
            .buses()
            .iter()
            .any(|b| b.id.eq_ignore_ascii_case(&auxiliary))
        {
            auxiliary.push('_');
        }
        let mut text = format!(
            "New Vsource._pio_coupled_{} phases=3 bus1={}.0 bus2={auxiliary}.1.2.3 basekv=1 pu=0 frequency={} basefreq={}",
            line.name,
            self.bus_ref(&line.bus_from, &line.terminal_map_from),
            num(net.base_frequency()),
            num(net.base_frequency())
        );
        for (i, value) in sequence.iter().enumerate() {
            let _ = write!(text, " z{i}=[{} {}]", num(value.re), num(value.im));
        }
        self.line_out(&text);
        // The third auxiliary node has no other attachment, imposing I3=0.
        // The first two ports retain both off-diagonal entries. Independent
        // series corrections restore the two original diagonal impedances.
        let to = self.bus_ref(&line.bus_to, &line.terminal_map_to);
        self.line_out(&format!("New Line.{} phases=2 bus1={auxiliary}.1.2 bus2={} length=1 units=m rmatrix=[{} | 0 {}] xmatrix=[{} | 0 {}] cmatrix=[0 | 0 0] basefreq={}",line.name,to,num(correction[0].re),num(correction[1].re),num(correction[0].im),num(correction[1].im),num(net.base_frequency())));
        self.warn(&super::C::EMIT_DSS_IMPEDANCE_EQUIVALENT,format!("line {}: full two-conductor impedance uses Vsource._pio_coupled_{} at the physical from end, an open auxiliary third port, and Line.{} at the physical to end; aggregate both elements for port power and loss",line.name,line.name,line.name));
    }
}
