//! Exact scalar line current variables used only by canonical compilers.
use crate::{DistLine, DistLineCode};
use num_complex::Complex64;

// Let u = scale * I. The auxiliary row enforces Vf - Vt = Z * u / scale,
// while the physical port rows give If = u / scale and It = -u / scale.
// The symmetric circuit stamp is
// [[0, 0, 1/s], [0, 0, -1/s], [1/s, -1/s, -Z/s²]].
// Two opposite series resistors and three shunts realize it without 1/Z
// entries in the nodal matrix. The impedance itself is never regularized.
pub(crate) struct ScalarCurrentForm {
    pub impedance: Complex64,
    pub scale: f64,
}

impl ScalarCurrentForm {
    pub fn for_line(line: &DistLine, code: &DistLineCode) -> Option<Self> {
        if code.n_conductors != 1
            || line.terminal_map_from.len() != 1
            || line.terminal_map_to.len() != 1
            || line.terminal_map_from != line.terminal_map_to
            || !line.extras.is_empty()
            || !code.extras.is_empty()
            || [&code.g_from, &code.g_to, &code.b_from, &code.b_to]
                .iter()
                .any(|m| m.as_slice() != [vec![0.0]])
            || code.r_series.len() != 1
            || code.x_series.len() != 1
            || code.r_series[0].len() != 1
            || code.x_series[0].len() != 1
        {
            return None;
        }
        let impedance = Complex64::new(code.r_series[0][0], code.x_series[0][0]) * line.length;
        // Select a numerically useful representation, never replace impedance.
        // More extreme and coupled circuits keep their existing projection.
        if !impedance.re.is_finite()
            || !impedance.im.is_finite()
            || !(1e-8..=1e-5).contains(&impedance.norm())
        {
            return None;
        }
        Some(Self {
            impedance,
            scale: 1.0,
        })
    }
    pub fn auxiliary_impedance(&self) -> Complex64 {
        -self.scale * self.scale / self.impedance
    }
}
