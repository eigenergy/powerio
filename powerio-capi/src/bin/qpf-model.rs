use std::ffi::{CStr, CString};

use powerio_capi::{
    PIO_ERRBUF_MIN, PioDcBranch, pio_dc_branches, pio_dc_n_buses, pio_network_free, pio_parse_file,
};

fn error_text(buffer: &[std::ffi::c_char]) -> String {
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn positive_dyadic(bits: u64) -> Result<(u64, i32), String> {
    let sign = bits >> 63;
    let exponent_bits = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    if sign != 0 || exponent_bits == 0x7ff || (exponent_bits == 0 && fraction == 0) {
        return Err(format!(
            "susceptance bits {bits:016x} are not finite and positive"
        ));
    }
    let (mut significand, mut exponent) = if exponent_bits == 0 {
        (fraction, -1074)
    } else {
        ((1_u64 << 52) | fraction, exponent_bits - 1023 - 52)
    };
    while significand & 1 == 0 {
        significand >>= 1;
        exponent += 1;
    }
    Ok((significand, exponent))
}

fn render_model(path: &str) -> Result<String, String> {
    let path = CString::new(path).map_err(|_| "case path contains a NUL byte".to_owned())?;
    let mut error = [0 as std::ffi::c_char; PIO_ERRBUF_MIN];
    let network = unsafe {
        pio_parse_file(
            path.as_ptr(),
            std::ptr::null(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if network.is_null() {
        return Err(error_text(&error));
    }
    let result = unsafe {
        let buses = pio_dc_n_buses(network, error.as_mut_ptr(), error.len());
        if buses < 0 {
            Err(error_text(&error))
        } else {
            let count = pio_dc_branches(
                network,
                std::ptr::null_mut(),
                0,
                error.as_mut_ptr(),
                error.len(),
            );
            if count < 0 {
                Err(error_text(&error))
            } else {
                let mut branches = vec![
                    PioDcBranch {
                        from_index: 0,
                        to_index: 0,
                        source_row: 0,
                        susceptance_bits: 0,
                    };
                    count as usize
                ];
                let filled = pio_dc_branches(
                    network,
                    branches.as_mut_ptr(),
                    branches.len(),
                    error.as_mut_ptr(),
                    error.len(),
                );
                if filled != count {
                    Err(if filled < 0 {
                        error_text(&error)
                    } else {
                        "DC model changed between count and fill".to_owned()
                    })
                } else {
                    let mut output = format!("QPFMODEL 1 N {buses} M {count} BRANCHES\n");
                    for branch in branches {
                        let (significand, exponent) = positive_dyadic(branch.susceptance_bits)?;
                        output.push_str(&format!(
                            "{} {} {} {}\n",
                            branch.from_index, branch.to_index, significand, exponent
                        ));
                    }
                    output.push_str("END\n");
                    Ok(output)
                }
            }
        }
    };
    unsafe { pio_network_free(network) };
    result
}

fn run(path: &str) -> Result<(), String> {
    print!("{}", render_model(path)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::positive_dyadic;

    #[test]
    fn dyadic_round_trips_normal_and_subnormal_values() {
        for value in [1.0_f64, 16.0_f64, 1.0_f64 / 0.0576_f64] {
            let (significand, exponent) = positive_dyadic(value.to_bits()).unwrap();
            assert_eq!(significand & 1, 1);
            let reconstructed = (significand as f64) * 2.0_f64.powi(exponent);
            assert_eq!(reconstructed.to_bits(), value.to_bits());
        }
        assert_eq!(
            positive_dyadic(f64::from_bits(1).to_bits()).unwrap(),
            (1, -1074)
        );
    }

    #[test]
    fn dyadic_rejects_nonpositive_and_nonfinite_values() {
        for value in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert!(positive_dyadic(value.to_bits()).is_err());
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: qpf-model <case>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: qpf-model <case>");
        std::process::exit(2);
    }
    if let Err(message) = run(&path) {
        eprintln!("qpf-model: {message}");
        std::process::exit(1);
    }
}
