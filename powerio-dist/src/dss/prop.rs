//! OpenDSS class and property name tables, in definition order.
//!
//! Names and order come from each class's DefineProperties in the OpenDSS
//! source (epri-dev/OpenDSS-C): the order fixes both positional property
//! assignment and abbreviation resolution. Lookup is case insensitive, exact
//! match first, then the first name in definition order with the query as a
//! prefix (THashList::FindAbbrev). Every class list ends with the inherited
//! properties: PD elements add normamps..repair, PC elements add spectrum,
//! and every circuit element adds basefreq, enabled, like.

/// One OpenDSS object class: canonical name plus ordered property names.
pub struct DssClass {
    pub name: &'static str,
    pub props: &'static [&'static str],
}

macro_rules! class {
    ($ident:ident, $name:literal, [$($p:literal),* $(,)?]) => {
        pub static $ident: DssClass = DssClass { name: $name, props: &[$($p),*] };
    };
}

class!(
    LINE,
    "line",
    [
        "bus1",
        "bus2",
        "linecode",
        "length",
        "phases",
        "r1",
        "x1",
        "r0",
        "x0",
        "c1",
        "c0",
        "rmatrix",
        "xmatrix",
        "cmatrix",
        "switch",
        "rg",
        "xg",
        "rho",
        "geometry",
        "units",
        "spacing",
        "wires",
        "earthmodel",
        "cncables",
        "tscables",
        "b1",
        "b0",
        "seasons",
        "ratings",
        "linetype",
        // inherited
        "normamps",
        "emergamps",
        "faultrate",
        "pctperm",
        "repair",
        "basefreq",
        "enabled",
        "like",
    ]
);

class!(
    LINECODE,
    "linecode",
    [
        "nphases",
        "r1",
        "x1",
        "r0",
        "x0",
        "c1",
        "c0",
        "units",
        "rmatrix",
        "xmatrix",
        "cmatrix",
        "basefreq",
        "normamps",
        "emergamps",
        "faultrate",
        "pctperm",
        "repair",
        "kron",
        "rg",
        "xg",
        "rho",
        "neutral",
        "b1",
        "b0",
        "seasons",
        "ratings",
        "linetype",
        // inherited
        "like",
    ]
);

class!(
    LOAD,
    "load",
    [
        "phases",
        "bus1",
        "kv",
        "kw",
        "pf",
        "model",
        "yearly",
        "daily",
        "duty",
        "growth",
        "conn",
        "kvar",
        "rneut",
        "xneut",
        "status",
        "class",
        "vminpu",
        "vmaxpu",
        "vminnorm",
        "vminemerg",
        "xfkva",
        "allocationfactor",
        "kva",
        "%mean",
        "%stddev",
        "cvrwatts",
        "cvrvars",
        "kwh",
        "kwhdays",
        "cfactor",
        "cvrcurve",
        "numcust",
        "zipv",
        "%seriesrl",
        "relweight",
        "puxharm",
        "xrharm",
        // inherited
        "spectrum",
        "basefreq",
        "enabled",
        "like",
    ]
);

class!(
    TRANSFORMER,
    "transformer",
    [
        "phases",
        "windings",
        "wdg",
        "bus",
        "conn",
        "kv",
        "kva",
        "tap",
        "%r",
        "rneut",
        "xneut",
        "buses",
        "conns",
        "kvs",
        "kvas",
        "taps",
        "xhl",
        "xht",
        "xlt",
        "xscarray",
        "thermal",
        "n",
        "m",
        "flrise",
        "hsrise",
        "%loadloss",
        "%noloadloss",
        "normhkva",
        "emerghkva",
        "sub",
        "maxtap",
        "mintap",
        "numtaps",
        "subname",
        "%imag",
        "ppm_antifloat",
        "%rs",
        "bank",
        "xfmrcode",
        "xrconst",
        "x12",
        "x13",
        "x23",
        "leadlag",
        "wdgcurrents",
        "core",
        "rdcohms",
        "seasons",
        "ratings",
        // inherited
        "normamps",
        "emergamps",
        "faultrate",
        "pctperm",
        "repair",
        "basefreq",
        "enabled",
        "like",
    ]
);

class!(
    VSOURCE,
    "vsource",
    [
        "bus1",
        "basekv",
        "pu",
        "angle",
        "frequency",
        "phases",
        "mvasc3",
        "mvasc1",
        "x1r1",
        "x0r0",
        "isc3",
        "isc1",
        "r1",
        "x1",
        "r0",
        "x0",
        "scantype",
        "sequence",
        "bus2",
        "z1",
        "z0",
        "z2",
        "puz1",
        "puz0",
        "puz2",
        "basemva",
        "yearly",
        "daily",
        "duty",
        "model",
        "puzideal",
        // inherited
        "spectrum",
        "basefreq",
        "enabled",
        "like",
    ]
);

class!(
    CAPACITOR,
    "capacitor",
    [
        "bus1",
        "bus2",
        "phases",
        "kvar",
        "kv",
        "conn",
        "cmatrix",
        "cuf",
        "r",
        "xl",
        "harm",
        "numsteps",
        "states",
        // inherited
        "normamps",
        "emergamps",
        "faultrate",
        "pctperm",
        "repair",
        "basefreq",
        "enabled",
        "like",
    ]
);

class!(
    GENERATOR,
    "generator",
    [
        "phases",
        "bus1",
        "kv",
        "kw",
        "pf",
        "kvar",
        "model",
        "vminpu",
        "vmaxpu",
        "yearly",
        "daily",
        "duty",
        "dispmode",
        "dispvalue",
        "conn",
        "rneut",
        "xneut",
        "status",
        "class",
        "vpu",
        "maxkvar",
        "minkvar",
        "pvfactor",
        "forceon",
        "kva",
        "mva",
        "xd",
        "xdp",
        "xdpp",
        "h",
        "d",
        "usermodel",
        "userdata",
        "shaftmodel",
        "shaftdata",
        "dutystart",
        "debugtrace",
        "balanced",
        "xrdp",
        "usefuel",
        "fuelkwh",
        "%fuel",
        "%reserve",
        "refuel",
        "dynamiceq",
        "dynout",
        // inherited
        "spectrum",
        "basefreq",
        "enabled",
        "like",
    ]
);

class!(
    SWTCONTROL,
    "swtcontrol",
    [
        "switchedobj",
        "switchedterm",
        "action",
        "lock",
        "delay",
        "normal",
        "state",
        "reset",
        // inherited
        "basefreq",
        "enabled",
        "like",
    ]
);

class!(
    REGCONTROL,
    "regcontrol",
    [
        "transformer",
        "winding",
        "vreg",
        "band",
        "ptratio",
        "ctprim",
        "r",
        "x",
        "bus",
        "delay",
        "reversible",
        "revvreg",
        "revband",
        "revr",
        "revx",
        "tapdelay",
        "debugtrace",
        "maxtapchange",
        "inversetime",
        "tapwinding",
        "vlimit",
        "ptphase",
        "revthreshold",
        "revdelay",
        "revneutral",
        "eventlog",
        "remoteptratio",
        "tapnum",
        "reset",
        "ldc_z",
        "rev_z",
        "cogen",
        // inherited
        "basefreq",
        "enabled",
        "like",
    ]
);

/// The Phase A classes with property tables. Anything else parses into the
/// raw layer untyped.
static CLASSES: &[&DssClass] = &[
    &LINE,
    &LINECODE,
    &LOAD,
    &TRANSFORMER,
    &VSOURCE,
    &CAPACITOR,
    &GENERATOR,
    &SWTCONTROL,
    &REGCONTROL,
];

/// Case insensitive exact class name lookup (`circuit` is handled by the
/// command layer, not here).
pub fn class_by_name(name: &str) -> Option<&'static DssClass> {
    CLASSES
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
        .copied()
}

impl DssClass {
    /// Property lookup: exact case insensitive match first, then the first
    /// property in definition order that starts with the query.
    pub fn prop_index(&self, query: &str) -> Option<usize> {
        let q = query.to_ascii_lowercase();
        self.props
            .iter()
            .position(|p| *p == q)
            .or_else(|| self.props.iter().position(|p| p.starts_with(&q)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_beats_prefix() {
        // "r1" is exact even though "r0" or "rmatrix" share the prefix.
        assert_eq!(LINE.prop_index("r1"), Some(5));
        assert_eq!(LINE.prop_index("R1"), Some(5));
    }

    #[test]
    fn first_prefix_match_in_definition_order() {
        // "r" has no exact match; the first r* property in order is r1.
        assert_eq!(LINE.prop_index("r"), Some(5));
        // "rm" picks rmatrix, not rg or rho.
        assert_eq!(LINE.prop_index("rm"), Some(11));
        // "norm" picks normamps from the inherited tail.
        assert_eq!(LINE.prop_index("norm"), Some(30));
    }

    #[test]
    fn percent_properties() {
        assert_eq!(TRANSFORMER.prop_index("%R"), Some(8));
        assert_eq!(TRANSFORMER.prop_index("%Rs"), Some(36));
        assert_eq!(TRANSFORMER.prop_index("%loadloss"), Some(25));
    }

    #[test]
    fn class_lookup() {
        assert!(class_by_name("Line").is_some());
        assert!(class_by_name("LINECODE").is_some());
        assert!(class_by_name("reactor").is_none());
    }

    #[test]
    fn unknown_property() {
        assert_eq!(LINE.prop_index("zzz"), None);
    }
}
