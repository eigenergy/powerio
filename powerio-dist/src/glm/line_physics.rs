use crate::DistLineCode;
use num_complex::Complex64;

/// The pinned FBS implementations set d=a. A physical pi line needs distinct
/// a=I+Z*Y and d=I+Y*Z, where Y is one end's shunt. Only commuting matrices agree.
/// Callers validate square, finite matrices before calling this function.
pub(super) fn fbs_shunts_commute(code: &DistLineCode) -> bool {
    let n = code.n_conductors;
    for i in 0..n {
        for j in 0..n {
            let mut zy = Complex64::default();
            let mut yz = zy;
            let mut scale = 0.0;
            for k in 0..n {
                let a = Complex64::new(code.r_series[i][k], code.x_series[i][k])
                    * Complex64::new(code.g_from[k][j], code.b_from[k][j]);
                let b = Complex64::new(code.g_from[i][k], code.b_from[i][k])
                    * Complex64::new(code.r_series[k][j], code.x_series[k][j]);
                zy += a;
                yz += b;
                scale += a.norm() + b.norm();
            }
            if !scale.is_finite()
                || !zy.re.is_finite()
                || !zy.im.is_finite()
                || !yz.re.is_finite()
                || !yz.im.is_finite()
                || (zy - yz).norm() > 32.0 * f64::EPSILON * scale
            {
                return false;
            }
        }
    }
    true
}
