//! Fixed open Line ports use the native zero-current Schur complement.
use super::{BusSpec, C, DistLine, DistLineCode, RawDss, Reader};
use crate::{DistShunt, DistSwitch};
use num_complex::Complex64 as Complex;

type Matrix = Vec<Vec<Complex>>;

fn inverse(mut matrix: Matrix) -> Result<Matrix, String> {
    let n = matrix.len();
    let mut result = vec![vec![Complex::default(); n]; n];
    for (i, row) in result.iter_mut().enumerate() {
        row[i] = Complex::new(1.0, 0.0);
    }
    for k in 0..n {
        let pivot = (k..n)
            .max_by(|&a, &b| matrix[a][k].norm_sqr().total_cmp(&matrix[b][k].norm_sqr()))
            .unwrap();
        if matrix[pivot][k] == Complex::default() {
            return Err("open Line series impedance is singular".into());
        }
        matrix.swap(k, pivot);
        result.swap(k, pivot);
        let diagonal = matrix[k][k];
        for j in 0..n {
            matrix[k][j] /= diagonal;
            result[k][j] /= diagonal;
        }
        let row = matrix[k].clone();
        let inv_row = result[k].clone();
        for i in 0..n {
            if i == k {
                continue;
            }
            let factor = matrix[i][k];
            for j in 0..n {
                matrix[i][j] -= factor * row[j];
                result[i][j] -= factor * inv_row[j];
            }
        }
    }
    Ok(result)
}

fn primitive(code: &DistLineCode, length: f64, closed: &[Vec<bool>; 2]) -> Result<Matrix, String> {
    let n = code.r_series.len();
    let impedance = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| Complex::new(code.r_series[i][j], code.x_series[i][j]) * length)
                .collect()
        })
        .collect();
    let series = inverse(impedance)?;
    let mut matrix = vec![vec![Complex::default(); 2 * n]; 2 * n];
    for i in 0..n {
        for j in 0..n {
            // Native TCMatrix is symmetric. Avoid introducing roundoff
            // asymmetry from a general inverse into the typed model.
            let y = series[i.max(j)][i.min(j)];
            matrix[i][j] = y + Complex::new(code.g_from[i][j], code.b_from[i][j]) * length;
            matrix[n + i][n + j] = y + Complex::new(code.g_to[i][j], code.b_to[i][j]) * length;
            matrix[i][n + j] = -y;
            matrix[n + i][j] = -y;
        }
    }
    let mut eliminated = vec![false; 2 * n];
    // TDSSCktElement.DoYprimCalcs eliminates terminals then conductors,
    // and adds EPSILON=1e-12 S on every external diagonal when any opens.
    for k in 0..2 * n {
        if closed[k / n][k % n] {
            continue;
        }
        let diagonal = if matrix[k][k] == Complex::default() {
            Complex::new(1e-12, 0.0)
        } else {
            matrix[k][k]
        };
        eliminated[k] = true;
        for i in 0..2 * n {
            if eliminated[i] {
                continue;
            }
            for j in i..2 * n {
                if !eliminated[j] {
                    let reduction = matrix[i][k] * matrix[k][j] / diagonal;
                    matrix[i][j] -= reduction;
                    matrix[j][i] = matrix[i][j];
                }
            }
        }
        matrix[k].fill(Complex::default());
        for row in &mut matrix {
            row[k] = Complex::default();
        }
    }
    for (i, row) in matrix.iter_mut().enumerate() {
        row[i] += 1e-12;
    }
    if matrix
        .iter()
        .flatten()
        .any(|v| !v.re.is_finite() || !v.im.is_finite())
    {
        return Err("open Line admittance is nonfinite".into());
    }
    Ok(matrix)
}

impl Reader<'_> {
    pub(super) fn open_line(
        &mut self,
        raw: &RawDss,
        line: &DistLine,
        code: &DistLineCode,
        closed: &[Vec<bool>; 2],
    ) -> Result<(), String> {
        let matrix = primitive(code, line.length, closed)?;
        let n = line.terminal_map_from.len();
        let base = format!("_pio_open_line_{}", line.name);
        let mut stem = base.clone();
        let mut suffix = 0;
        // Reserve the whole generated family against declared identifiers and
        // bus literals, including devices that the reader has not visited yet.
        while self
            .buses
            .keys()
            .any(|s| s.contains(&stem.to_ascii_lowercase()))
            || raw.objects.iter().any(|o| {
                o.name
                    .to_ascii_lowercase()
                    .contains(&stem.to_ascii_lowercase())
                    || o.props.iter().any(|p| {
                        p.value
                            .text
                            .to_ascii_lowercase()
                            .contains(&stem.to_ascii_lowercase())
                    })
            })
            || raw.buscoords.iter().any(|b| {
                b.bus
                    .to_ascii_lowercase()
                    .contains(&stem.to_ascii_lowercase())
            })
        {
            suffix += 1;
            stem = format!("{base}_{suffix}");
        }
        let bus = format!("{stem}_ports");
        let nodes = (1..=2 * n)
            .map(|i| i32::try_from(i).expect("Line has at most 32 conductors"))
            .collect();
        let terms = self.terminals(
            &BusSpec {
                name: bus.clone(),
                nodes,
            },
            2 * n,
            2 * n,
            2 * n,
        );
        let mut shunt = DistShunt::new(
            &stem,
            &bus,
            terms.clone(),
            matrix
                .iter()
                .map(|r| r.iter().map(|v| v.re).collect())
                .collect(),
            matrix
                .iter()
                .map(|r| r.iter().map(|v| v.im).collect())
                .collect(),
        );
        // Provenance identifies the boundary of the multi-element equivalent.
        // Keep it off contact extras, which describe retained impedances.
        shunt.extras.insert(
            "dss_original_element".into(),
            format!("line.{}", line.name).into(),
        );
        shunt.extras.insert(
            "dss_external_contacts".into(),
            serde_json::json!([format!("{stem}_from"), format!("{stem}_to")]),
        );
        self.net.shunts_mut().push(shunt);
        for (label, endpoint, original, offset) in [
            ("from", &line.bus_from, &line.terminal_map_from, 0),
            ("to", &line.bus_to, &line.terminal_map_to, n),
        ] {
            let mut contact = DistSwitch::new(
                format!("{stem}_{label}"),
                endpoint,
                &bus,
                original.clone(),
                terms[offset..offset + n].to_vec(),
                false,
            );
            contact.i_max.clone_from(&line.i_max);
            self.net.switches_mut().push(contact);
        }
        self.warn(&C::READ_DSS_RETAINED_SOURCE_ONLY,format!("Line.{}: fixed open ports use the native zero-current reduced admittance, including 1e-12 S diagonal stabilization, with ideal external contacts; effective linecode retained and line metadata remains source-only",line.name));
        Ok(())
    }
}
