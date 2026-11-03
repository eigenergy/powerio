//! Pinned FBS line hybrid equations, including the native d=a convention.
use super::{Reader, allocate_name};
use crate::glm::ObjectKey;
use crate::{DistBus, DistLineCode, DistLocationTarget, DistShunt, DistSwitch};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

impl Reader<'_> {
    pub(super) fn warn_native_line_reporting(&mut self, key: ObjectKey) {
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_NATIVE_LINE_DISCREPANCY,
            format!("line {}: native NR source-end current/loss reporting differs from the physical pi-line law for noncommuting impedance/capacitance matrices; the network preserves the physical NR solve circuit", self.names[&key])));
    }

    pub(super) fn fbs_line(
        &mut self,
        key: ObjectKey,
        phases: &[usize],
        code: &DistLineCode,
        length: f64,
    ) -> Result<(), Error> {
        let n = phases.len();
        let z = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| Complex64::new(code.r_series[i][j], code.x_series[i][j]) * length)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        // y is one end's shunt, so a=I+Z*y and c=2*y+y*Z*y.
        let y = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| Complex64::new(code.g_from[i][j], code.b_from[i][j]) * length)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let scale = z.iter().flatten().map(|v| v.norm()).fold(0.0_f64, f64::max);
        if !scale.is_finite() || scale <= 0.0 {
            return Err(self.error(
                key,
                "native FBS line requires finite positive impedance scaling",
            ));
        }
        let product = |left: &[Vec<Complex64>], right: &[Vec<Complex64>]| {
            (0..n)
                .map(|i| {
                    (0..n)
                        .map(|j| (0..n).map(|k| left[i][k] * right[k][j]).sum::<Complex64>())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let zy = product(&z, &y);
        let yzy = product(&y, &zy);
        let mut matrix = vec![vec![Complex64::default(); 3 * n]; 3 * n];
        for i in 0..n {
            for j in 0..n {
                let identity = if i == j { 1.0 } else { 0.0 };
                let a = zy[i][j] + identity;
                // External rows: Iin=c*Vto+a*Iout and Ito=-Iout.
                matrix[i][n + j] = 2.0 * y[i][j] + yzy[i][j];
                matrix[i][2 * n + j] = a / scale;
                matrix[n + i][2 * n + j] = Complex64::new(-identity / scale, 0.0);
                // Auxiliary voltages u=scale*Iout impose Vf=a*Vt+Z*Iout.
                matrix[2 * n + i][j] = Complex64::new(-identity / scale, 0.0);
                matrix[2 * n + i][n + j] = a / scale;
                matrix[2 * n + i][2 * n + j] = z[i][j] / scale / scale;
            }
        }
        if matrix
            .iter()
            .flatten()
            .any(|v| !v.re.is_finite() || !v.im.is_finite())
        {
            return Err(self.error(key, "native FBS line admittance overflows"));
        }
        let from = self.ordinary_link_endpoint(key, "from", phases)?;
        let to = self.ordinary_link_endpoint(key, "to", phases)?;
        let original = self.names[&key].clone();
        let bus = allocate_name(&mut self.identities, &format!("{original}__glm_ports"));
        let terms = (1..=3 * n).map(|i| i.to_string()).collect::<Vec<_>>();
        self.net.buses_mut().push(DistBus::new(&bus, terms.clone()));
        self.net.shunts_mut().push(DistShunt::new(
            &original,
            &bus,
            terms,
            matrix
                .iter()
                .map(|r| r.iter().map(|v| v.re).collect())
                .collect(),
            matrix
                .iter()
                .map(|r| r.iter().map(|v| v.im).collect())
                .collect(),
        ));
        self.attach(key, DistLocationTarget::Shunt, original.clone());
        for (label, endpoint, offset) in [("from", from, 0), ("to", to, n)] {
            let name = allocate_name(&mut self.identities, &format!("{original}__glm_{label}"));
            self.net.switches_mut().push(DistSwitch::new(
                &name,
                endpoint,
                &bus,
                phases.iter().map(|p| (p + 1).to_string()).collect(),
                (1 + offset..=n + offset).map(|i| i.to_string()).collect(),
                false,
            ));
            self.attach(key, DistLocationTarget::Switch, name);
        }
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_LINE_EQUIVALENT,
            format!("line {original}: native FBS d=a equations use a {}-terminal admittance with scaled current variables and ideal external contacts; this preserves the nonreciprocal native solve law rather than a physical pi circuit; the materialized configuration is retained as a linecode; remaining line metadata stays source-only",3*n)));
        Ok(())
    }
}
