//! Exact native FBS delta-delta hybrid equations as a conductor admittance.
use super::{Reader, allocate_name};
use crate::glm::ObjectKey;
use crate::{DistBus, DistLocationTarget, DistShunt, DistSwitch};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

impl Reader<'_> {
    pub(super) fn fbs_delta_delta(
        &mut self,
        key: ObjectKey,
        cfg: ObjectKey,
        phases: &[usize],
    ) -> Result<(), Error> {
        if phases != [0, 1, 2] {
            return Err(self.error(key, "FBS DELTA_DELTA requires all three phases"));
        }
        let primary = self.positive_geometry(cfg, "primary_voltage")?;
        let secondary = self.positive_geometry(cfg, "secondary_voltage")?;
        let rating = self.transformer_rating(cfg, true)?;
        let z = self
            .quantity(cfg, "impedance")
            .ok_or_else(|| self.error(cfg, "FBS DELTA_DELTA requires series impedance"))?;
        if z.re <= 0.0 || z.im <= 0.0 {
            return Err(self.error(
                cfg,
                "native transformer requires positive series resistance and reactance",
            ));
        }
        let ratio = primary / secondary;
        let scale = secondary * secondary / rating;
        if !ratio.is_finite() || ratio <= 0.0 || !scale.is_finite() || scale <= 0.0 {
            return Err(self.error(cfg, "FBS delta transformer scaling overflows or underflows"));
        }
        let from = self.ordinary_link_endpoint(key, "from", phases)?;
        let to = self.ordinary_link_endpoint(key, "to", phases)?;
        let original = self.names[&key].clone();
        let bus_name = allocate_name(&mut self.identities, &format!("{original}__glm_ports"));
        let terms = (1..=9).map(|i| i.to_string()).collect::<Vec<_>>();
        let bus = DistBus::new(&bus_name, terms.clone());
        // The last three voltages are scaled current variables u=scale*Iout.
        // Their KCL rows impose Vto=P*Vfrom/ratio - Z*T*Iout, where P
        // removes common mode and T maps [Ia,Ib,Ic] to [Ia,Ib,-Ia-Ib].
        // External rows give Iin=Iout/ratio, even for nonzero common-mode
        // current. A physical delta winding does not have this native law.
        self.net.buses_mut().push(bus);
        let mut admittance = vec![vec![Complex64::default(); 9]; 9];
        for phase in 0..3 {
            admittance[phase][6 + phase] = Complex64::new(1.0 / ratio / scale, 0.0);
            admittance[3 + phase][6 + phase] = Complex64::new(-1.0 / scale, 0.0);
            admittance[6 + phase][3 + phase] = Complex64::new(1.0 / scale, 0.0);
            for (other, entry) in admittance[6 + phase][..3].iter_mut().enumerate() {
                let projection = if phase == other {
                    2.0 / 3.0
                } else {
                    -1.0 / 3.0
                };
                *entry = Complex64::new(-projection / ratio / scale, 0.0);
            }
        }
        admittance[6][6] = z / scale;
        admittance[7][7] = z / scale;
        admittance[8][6] = -z / scale;
        admittance[8][7] = -z / scale;
        if admittance
            .iter()
            .flatten()
            .any(|v| !v.re.is_finite() || !v.im.is_finite())
        {
            return Err(self.error(cfg, "FBS delta transformer admittance overflows"));
        }
        self.net.shunts_mut().push(DistShunt::new(
            &original,
            &bus_name,
            terms,
            admittance
                .iter()
                .map(|r| r.iter().map(|v| v.re).collect())
                .collect(),
            admittance
                .iter()
                .map(|r| r.iter().map(|v| v.im).collect())
                .collect(),
        ));
        self.attach(key, DistLocationTarget::Shunt, original.clone());
        for (label, endpoint, offset) in [("from", from, 0), ("to", to, 3)] {
            let name = allocate_name(&mut self.identities, &format!("{original}__glm_{label}"));
            self.net.switches_mut().push(DistSwitch::new(
                &name,
                endpoint,
                &bus_name,
                (1..=3).map(|i| i.to_string()).collect(),
                (1 + offset..=3 + offset).map(|i| i.to_string()).collect(),
                false,
            ));
            self.attach(key, DistLocationTarget::Switch, name);
        }
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_TRANSFORMER_EQUIVALENT,
            format!("{original}: native FBS DELTA_DELTA is a nine-terminal admittance with three auxiliary current variables and ideal external contacts; it blocks common-mode voltage but transfers common-mode current, unlike a physical delta winding; nameplate ratings remain source-only")));
        Ok(())
    }
}
