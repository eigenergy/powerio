//! IndMach012a fixed-slip snapshot, represented by its full sequence admittance.
//! Equations/defaults follow the pinned native Pascal oracle in evals/gridlabd.
use super::{DistShunt, Props, RawDss, RawObject, Reader, bus_spec};
use crate::dss::lex::Scanner;
use num_complex::Complex64;

impl Reader<'_> {
    pub(super) fn fixed_motor(&mut self, object: &RawObject, raw: &RawDss) -> bool {
        if object
            .get("model")
            .is_none_or(|v| v.to_i64(Some(self.vars)) != Ok(6))
        {
            return false;
        }
        match self.motor_shunt(object, raw) {
            Ok(shunt) => {
                self.net.shunts_mut().push(shunt);
                true
            }
            Err(reason) => {
                self.warn(
                    &super::C::READ_DSS_RETAINED_SOURCE_ONLY,
                    format!("generator {}: {reason}", object.name),
                );
                false
            }
        }
    }

    fn motor_shunt(&mut self, object: &RawObject, raw: &RawDss) -> Result<DistShunt, String> {
        if raw
            .generator_copies
            .contains(&object.name.to_ascii_lowercase())
        {
            return Err("MakeLike user-library state is unresolved".into());
        }
        if raw.commands.iter().any(|command| {
            matches!(
                command.verb.as_str(),
                "enable" | "disable" | "open" | "close"
            ) && Scanner::new(&command.args, Some(self.vars))
                .next_param()
                .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("generator."))
        }) {
            return Err("generator terminal/enable command state is unresolved".into());
        }
        let motor = Motor::from_object(object)?;
        let props = Props::new(object);
        if props.get("enabled").is_some_and(|v| !v.to_bool()) {
            return Err("disabled user-library motor state is source-only".into());
        }
        if props
            .get("phases")
            .is_some_and(|v| v.to_i64(Some(self.vars)) != Ok(3))
            || props.get("conn").is_none_or(|v| {
                !matches!(v.text.to_ascii_lowercase().as_str(), "delta" | "d" | "ll")
            })
        {
            return Err("only three-phase delta IndMach012a is qualified".into());
        }
        let number = |key| {
            props
                .get(key)
                .ok_or_else(|| format!("explicit motor {key} is required"))?
                .to_f64(Some(self.vars))
                .map_err(|_| format!("invalid motor {key}"))
        };
        let (kv, kva) = (number("kv")?, number("kva")?);
        if !kv.is_finite() || kv <= 0.0 || !kva.is_finite() || kva <= 0.0 {
            return Err("motor bases must be positive and finite".into());
        }
        let matrix = motor.matrix(kv * kv / kva * 1000.0)?;
        let bus = bus_spec(props.get("bus1"), "");
        if bus.name.is_empty()
            || bus.nodes.len() > 3
            || bus.nodes.iter().any(|&n| n < 0 || n == i32::MAX)
        {
            return Err("invalid motor terminal specification".into());
        }
        let map = self.terminals(&bus, 3, 3, 3);
        let mut name = object.name.clone();
        while self
            .net
            .shunts()
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(&name))
        {
            name.push('_');
        }
        let mut shunt = DistShunt::new(
            name,
            bus.name,
            map,
            matrix
                .iter()
                .map(|row| row.iter().map(|v| v.re).collect())
                .collect(),
            matrix
                .iter()
                .map(|row| row.iter().map(|v| v.im).collect())
                .collect(),
        );
        shunt.extras.insert(
            "dss_original_element".into(),
            format!("generator.{}", object.name).into(),
        );
        Ok(shunt)
    }
}

struct Motor {
    impedance: [f64; 5],
    slip: f64,
    max_slip: f64,
    fixed: bool,
}
impl Default for Motor {
    fn default() -> Self {
        Self {
            impedance: [0.0053, 0.106, 0.007, 0.12, 4.0],
            slip: -0.007,
            max_slip: 0.1,
            fixed: false,
        }
    }
}
impl Motor {
    fn from_object(object: &RawObject) -> Result<Self, String> {
        let mut motor = None;
        for property in &object.props {
            match property.name.as_deref().unwrap_or("") {
                "usermodel" => {
                    if !property.value.text.eq_ignore_ascii_case("IndMach012a") {
                        return Err(
                            "user library is outside the qualified IndMach012a profile".into()
                        );
                    }
                    motor = Some(Motor::default());
                }
                "userdata" => {
                    // Native Edit does nothing until a library instance exists.
                    if let Some(motor) = &mut motor {
                        motor.edit(&property.value.text)?;
                    }
                }
                "bus1" | "phases" | "conn" | "kv" | "kva" | "kw" | "kvar" | "pf" | "model"
                | "enabled" => {}
                key => {
                    return Err(format!(
                        "motor property {key} is outside the qualified static profile"
                    ));
                }
            }
        }
        let motor = motor.ok_or("missing IndMach012a library")?;
        if !motor.fixed {
            return Err("variable-slip motor requires a nonlinear electrical law".into());
        }
        Ok(motor)
    }

    fn edit(&mut self, text: &str) -> Result<(), String> {
        let mut scanner = Scanner::new(text, None);
        while let Some(property) = scanner.next_param() {
            let key = property.name.as_deref().unwrap_or("");
            if key == "option" {
                match property.value.text.to_ascii_lowercase().chars().next() {
                    Some('f') => self.fixed = true,
                    Some('v') => self.fixed = false,
                    Some('d' | 'n') => {}
                    _ => return Err("unrecognized motor option".into()),
                }
                continue;
            }
            // Abbreviated, positional and expression userdata remain source-only
            // until checked against this library's embedded legacy parser.
            let value: f64 = property
                .value
                .text
                .parse()
                .map_err(|_| "invalid motor userdata number")?;
            if !value.is_finite() {
                return Err("nonfinite motor userdata".into());
            }
            match key {
                "rs" => self.impedance[0] = value,
                "xs" => self.impedance[1] = value,
                "rr" => self.impedance[2] = value,
                "xr" => self.impedance[3] = value,
                "xm" => self.impedance[4] = value,
                "slip" => self.slip = value.clamp(-self.max_slip, self.max_slip),
                "maxslip" if value >= 0.0 => self.max_slip = value,
                _ => return Err(format!("unsupported motor userdata {key}")),
            }
        }
        Ok(())
    }

    fn matrix(&self, base: f64) -> Result<[[Complex64; 3]; 3], String> {
        let [rs, xs, rr, xr, xm] = self.impedance;
        let zs = Complex64::new(rs, xs) * base;
        let zr = Complex64::new(rr, xr) * base;
        let zm = Complex64::new(0.0, xm) * base;
        let admittance = |slip: f64| {
            let rl = if slip == 0.0 {
                zr.re * 1e6
            } else {
                zr.re * (1.0 - slip) / slip
            };
            let rotor = zr + rl;
            1.0 / (zs + zm * rotor / (zm + rotor))
        };
        let positive = admittance(self.slip);
        let negative = admittance(2.0 - self.slip);
        // mathutil.pas uses this truncated rotation and actually inverts A.
        // Conjugate(A)/3 is not its inverse; the difference is observable for
        // low-slip currents. Preserve the pinned library's current law.
        let rotation = 0.866_025_403;
        let a = Complex64::new(-0.5, rotation);
        let one = Complex64::new(1.0, 0.0);
        let third = one / 3.0;
        let inverse = Complex64::new(-1.0 / 6.0, 1.0 / (4.0 * rotation));
        let forward = [[one, one], [a.conj(), a], [a, a.conj()]];
        let reverse = [
            [third, inverse, inverse.conj()],
            [third, inverse.conj(), inverse],
        ];
        let mut matrix = [[Complex64::default(); 3]; 3];
        for (i, row) in matrix.iter_mut().enumerate() {
            for (j, entry) in row.iter_mut().enumerate() {
                *entry = forward[i][0] * positive * reverse[0][j]
                    + forward[i][1] * negative * reverse[1][j];
                if !entry.re.is_finite() || !entry.im.is_finite() {
                    return Err("singular motor equivalent".into());
                }
            }
        }
        Ok(matrix)
    }
}
