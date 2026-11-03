//! Native nominal Load power state, including UseActual assignment side effects.
use super::defaults::load;
use super::lex::{Value, VarMap};

#[allow(
    clippy::struct_excessive_bools,
    reason = "native specification flags and independent source provenance"
)]
#[derive(Clone, Debug)]
pub(super) struct LoadPower {
    pub kw: f64,
    pub kvar: f64,
    pub pf: f64,
    pub spec_kvar: bool,
    pub kw_written: bool,
    pub pf_written: bool,
    pub pf_specified: bool,
}

impl Default for LoadPower {
    fn default() -> Self {
        let mut value = Self {
            kw: load::KW,
            kvar: 0.0,
            pf: load::PF,
            spec_kvar: false,
            kw_written: false,
            pf_written: false,
            pf_specified: false,
        };
        value.finish();
        value
    }
}

impl LoadPower {
    pub fn property(&mut self, key: &str, value: &Value, vars: Option<&VarMap>) {
        let Ok(value) = value.to_f64(vars) else {
            return;
        };
        match key {
            "kw" => {
                self.kw = value;
                self.spec_kvar = false;
                self.kw_written = true;
            }
            "kvar" => {
                self.kvar = value;
                self.spec_kvar = true;
                self.pf_specified = false;
            }
            "pf" => {
                self.pf = value;
                self.pf_written = true;
                self.pf_specified = true;
            }
            _ => {}
        }
    }

    pub fn actual(&mut self, p: f64, q: f64) {
        self.kw = p;
        self.kvar = q;
        self.spec_kvar = !self.pf_specified;
        self.kw_written = true;
    }

    pub fn make_like(&mut self, source: &Self) {
        // Native Load.MakeLike copies nominal powers/specification, but leaves
        // the target's PFSpecified flag intact (including a fresh false flag).
        let specified = self.pf_specified;
        *self = source.clone();
        self.pf_specified = specified;
    }

    pub fn finish(&mut self) {
        if self.spec_kvar {
            let kva = self.kw.hypot(self.kvar);
            if kva > 0.0 {
                self.pf = self.kw / kva;
                if self.kw * self.kvar < 0.0 {
                    self.pf = -self.pf;
                }
            }
        } else {
            self.kvar = self.kw * (1.0 / (self.pf * self.pf) - 1.0).sqrt();
            if self.pf < 0.0 {
                self.kvar = -self.kvar;
            }
        }
    }
}
