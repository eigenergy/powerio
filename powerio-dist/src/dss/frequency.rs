//! Ordered executive frequency state, separate from retained option text.
//!
//! Pinned DSS C-API ExecOptions.pas cases 6, 53 and 73 distinguish solution,
//! circuit fundamental and process default frequency. CktElement constructors
//! inherit the circuit fundamental; Clear only discards circuit state.

use super::lex::{Value, VarMap};

// Native option order through the final frequency-setting option. Later
// options cannot advance positionally back into one of these three slots.
const OPTIONS: &[&str] = &[
    "type",
    "element",
    "hour",
    "sec",
    "year",
    "frequency",
    "stepsize",
    "mode",
    "random",
    "number",
    "time",
    "class",
    "object",
    "circuit",
    "editor",
    "tolerance",
    "maxiterations",
    "h",
    "loadmodel",
    "loadmult",
    "normvminpu",
    "normvmaxpu",
    "emergvminpu",
    "emergvmaxpu",
    "%mean",
    "%stddev",
    "ldcurve",
    "%growth",
    "genkw",
    "genpf",
    "capkvar",
    "addtype",
    "allowduplicates",
    "zonelock",
    "ueweight",
    "lossweight",
    "ueregs",
    "lossregs",
    "voltagebases",
    "algorithm",
    "trapezoidal",
    "autobuslist",
    "controlmode",
    "tracecontrol",
    "genmult",
    "defaultdaily",
    "defaultyearly",
    "allocationfactors",
    "cktmodel",
    "pricesignal",
    "pricecurve",
    "terminal",
    "basefrequency",
    "harmonics",
    "maxcontroliter",
    "bus",
    "datapath",
    "keeplist",
    "reduceoption",
    "demandinterval",
    "%normal",
    "diverbose",
    "casename",
    "markercode",
    "nodewidth",
    "log",
    "recorder",
    "overloadreport",
    "voltexceptionreport",
    "cfactors",
    "showexport",
    "numallociterations",
    "defaultbasefrequency",
];

pub(super) fn option_position(name: &str) -> usize {
    OPTIONS
        .iter()
        .position(|&s| s == name)
        .or_else(|| OPTIONS.iter().position(|s| s.starts_with(name)))
        .map_or(usize::MAX, |i| i + 1)
}

#[derive(Debug)]
pub(super) struct FrequencyState {
    default: f64,
    fundamental: Option<f64>,
    snapshot: Option<f64>,
}

impl Default for FrequencyState {
    fn default() -> Self {
        Self {
            default: super::defaults::BASE_FREQUENCY,
            fundamental: None,
            snapshot: None,
        }
    }
}

impl FrequencyState {
    pub fn clear(&mut self) {
        self.fundamental = None;
        self.snapshot = None;
    }

    pub fn new_circuit(&mut self) {
        self.fundamental = Some(self.default);
        self.snapshot = Some(self.default);
    }

    pub fn constructor(&self) -> f64 {
        self.fundamental.unwrap_or(self.default)
    }

    pub fn snapshot(&self) -> f64 {
        self.snapshot.unwrap_or(self.default)
    }

    pub fn assignment(&mut self, position: &mut usize, name: &str, value: &Value, vars: &VarMap) {
        *position = if name.is_empty() {
            position.saturating_add(1)
        } else {
            option_position(name)
        };
        if !matches!(*position, 6 | 53 | 73) {
            return;
        }
        // Keep invalid values invalid: the electrical resolver must refuse
        // them rather than substitute a usable default.
        let frequency = value.to_f64(Some(vars)).unwrap_or(f64::NAN);
        if *position == 73 {
            self.default = frequency;
        }
        if self.fundamental.is_some() {
            if matches!(*position, 53 | 73) {
                self.fundamental = Some(frequency);
            }
            self.snapshot = Some(frequency);
        }
    }
}

#[cfg(test)]
// Assignment-state checks compare exactly copied literals, not computed physics.
#[allow(clippy::float_cmp)]
mod tests {
    use super::super::raw::__parse_raw_with;

    #[test]
    fn clear_preserves_default_but_discards_circuit_and_solution_frequency() {
        let raw = __parse_raw_with(
            "Set defaultb=50\nNew Circuit.old\nSet basefrequency=55 frequency=70\nClear\nNew Circuit.new",
            "test.dss",
            &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
        );
        assert_eq!(raw.snapshot_frequency(), 50.0);
        let source = raw.find("vsource", "source").unwrap();
        assert_eq!(raw.source_replay(source).constructor_frequency, 50.0);
        assert_eq!(
            raw.options,
            [] as [(std::string::String, crate::dss::lex::Value); 0]
        );
    }

    #[test]
    fn includes_and_solve_options_keep_execution_order_and_retained_text() {
        let raw = __parse_raw_with(
            "New Circuit.c\nRedirect frequency.dss\nNew Vsource.second\nSolve year=0 55\nSet defaultdaily=default",
            "test.dss",
            &mut |_: &std::path::Path| Ok("Set basefrequency=50\nSet frequency=70".into()),
        );
        assert_eq!(raw.snapshot_frequency(), 55.0);
        assert_eq!(
            raw.source_replay(raw.find("vsource", "source").unwrap())
                .constructor_frequency,
            60.0
        );
        assert_eq!(
            raw.source_replay(raw.find("vsource", "second").unwrap())
                .constructor_frequency,
            50.0
        );
        assert!(
            raw.commands
                .iter()
                .any(|c| c.verb == "solve" && c.args == "year=0 55")
        );
    }

    #[test]
    fn invalid_frequency_is_not_replaced_with_a_valid_default() {
        let raw = __parse_raw_with(
            "New Circuit.c\nSet frequency=invalid",
            "test.dss",
            &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
        );
        assert!(raw.snapshot_frequency().is_nan());
    }
}
