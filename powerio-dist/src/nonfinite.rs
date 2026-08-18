//! Read side leniency for `null` at the bound fields.
//!
//! The multiconductor model writes a nonfinite `f64` as `"Infinity"`,
//! `"-Infinity"`, or `"NaN"` like every powerio document
//! (`powerio_diag::nonfinite`, threaded through the model's serde impls).
//! Before 0.9.0 serde_json wrote it as JSON `null`, and these modules keep
//! reading that `null` back by field role, so a payload an earlier writer
//! emitted still reads (#268): a `null` element in an upper bound means
//! unbounded above (+Inf); in a lower bound, unbounded below (-Inf); in a
//! length, not known (NaN). The role defaults are the PMD convention.
//!
//! serde `with` modules receive `&T`, so the serialize signatures take
//! references the lints would otherwise refuse.
//!
//! An `Option<f64>` field needs no module: a nonfinite value spells itself
//! as a string and round trips, and a pre-0.9 `null` reads back as `None`.
#![allow(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    clippy::ptr_arg
)]

use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn restore(v: Option<Vec<Option<f64>>>, missing: f64) -> Option<Vec<f64>> {
    v.map(|xs| xs.into_iter().map(|x| x.unwrap_or(missing)).collect())
}

fn restore_all(v: Vec<Option<f64>>, missing: f64) -> Vec<f64> {
    v.into_iter().map(|x| x.unwrap_or(missing)).collect()
}

/// `Option<Vec<f64>>` upper bounds: a `null` element reads as +Inf.
pub(crate) mod upper_bounds {
    use super::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<f64>>, s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<f64>>, D::Error> {
        let v = Option::<Vec<Option<f64>>>::deserialize(d)?;
        Ok(super::restore(v, f64::INFINITY))
    }
}

/// `Option<Vec<f64>>` lower bounds: a `null` element reads as -Inf.
pub(crate) mod lower_bounds {
    use super::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<f64>>, s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<f64>>, D::Error> {
        let v = Option::<Vec<Option<f64>>>::deserialize(d)?;
        Ok(super::restore(v, f64::NEG_INFINITY))
    }
}

/// Required `Vec<f64>` ratings: a `null` element reads as +Inf.
pub(crate) mod upper_limits {
    use super::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec<f64>, s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        let v = Vec::<Option<f64>>::deserialize(d)?;
        Ok(super::restore_all(v, f64::INFINITY))
    }
}

/// Required `f64` with a not-known state: `null` reads as NaN.
pub(crate) mod nan_scalar {
    use super::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
    }
}

/// The schema for a required field whose value may be `null`: the key must be
/// present, and `null` is how a nonfinite value spells itself.
///
/// `schemars(with = "Option<f64>")` cannot say this. Naming an `Option` type
/// there widens the type union *and* drops the field from the object's
/// `required` list, which would publish a schema that accepts a document
/// omitting the key — while [`nan_scalar`] still demands it, so the reader
/// would reject a document the schema called valid.
#[cfg(feature = "schema")]
pub(crate) fn nullable_number(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": ["number", "null"],
        "format": "double",
    })
}

#[cfg(test)]
mod tests {
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Probe {
        #[serde(with = "super::upper_bounds")]
        ub: Option<Vec<f64>>,
        #[serde(with = "super::lower_bounds")]
        lb: Option<Vec<f64>>,
        #[serde(with = "super::nan_scalar")]
        len: f64,
    }

    #[test]
    fn null_elements_read_as_signed_infinity_and_nan() {
        let p: Probe =
            serde_json::from_str(r#"{"ub":[1.0,null],"lb":[null,0.0],"len":null}"#).unwrap();
        assert_eq!(p.ub, Some(vec![1.0, f64::INFINITY]));
        assert_eq!(p.lb, Some(vec![f64::NEG_INFINITY, 0.0]));
        assert!(p.len.is_nan());
    }

    #[test]
    fn whole_null_bound_reads_as_absent() {
        let p: Probe = serde_json::from_str(r#"{"ub":null,"lb":null,"len":3.0}"#).unwrap();
        assert_eq!(p.ub, None);
        assert_eq!(p.lb, None);
    }

    #[test]
    fn write_read_write_is_stable() {
        let p = Probe {
            ub: Some(vec![500e3, f64::INFINITY]),
            lb: Some(vec![f64::NEG_INFINITY]),
            len: f64::NAN,
        };
        let text = serde_json::to_string(&p).unwrap();
        let back: Probe = serde_json::from_str(&text).unwrap();
        assert_eq!(back.ub, p.ub);
        assert_eq!(back.lb, p.lb);
        assert!(back.len.is_nan());
        assert_eq!(text, serde_json::to_string(&back).unwrap());
    }
    /// `serde(with = ...)` drops the implicit "missing means `None`" handling
    /// for an `Option` field, so every widened bound needs `default` beside it
    /// or a document that omits the key stops parsing.
    #[test]
    fn an_omitted_bound_still_reads_as_absent() {
        use crate::model::{DistLineCode, DistSwitch};

        let code = DistLineCode::new("lc", vec![vec![0.1]], vec![vec![0.2]]);
        let mut v = serde_json::to_value(&code).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("i_max");
        obj.remove("s_max");
        let back: DistLineCode = serde_json::from_value(v).expect("omitted bounds must parse");
        assert!(back.i_max.is_none() && back.s_max.is_none());

        let sw = DistSwitch::new("sw", "a", "b", vec!["1".into()], vec!["1".into()], false);
        let mut v = serde_json::to_value(&sw).unwrap();
        v.as_object_mut().unwrap().remove("i_max");
        let back: DistSwitch = serde_json::from_value(v).expect("omitted bound must parse");
        assert!(back.i_max.is_none());
    }

    /// The network type spells a nonfinite value as a string on every JSON
    /// route (the unified powerio convention), and the bound modules keep
    /// reading a pre-0.9 `null` element by field role.
    #[test]
    #[allow(clippy::float_cmp)]
    fn network_spells_nonfinite_as_strings_and_reads_legacy_null() {
        use crate::model::{DistLineCode, MulticonductorNetwork};

        let mut net = MulticonductorNetwork::named("nf");
        let mut code = DistLineCode::new("lc", vec![vec![0.1]], vec![vec![0.2]]);
        code.i_max = Some(vec![f64::INFINITY, 400.0]);
        net.linecodes.push(code);

        let text = serde_json::to_string(&net).unwrap();
        assert!(text.contains(r#""i_max":["Infinity",400.0]"#), "{text}");

        let back: MulticonductorNetwork = serde_json::from_str(&text).unwrap();
        assert_eq!(back.linecodes[0].i_max, Some(vec![f64::INFINITY, 400.0]));
        assert_eq!(serde_json::to_string(&back).unwrap(), text);

        // A pre-0.9 writer spelled the element `null`; the role default
        // (+Inf for an ampacity) still applies on read.
        let legacy = text.replace(r#""i_max":["Infinity",400.0]"#, r#""i_max":[null,400.0]"#);
        let back: MulticonductorNetwork = serde_json::from_str(&legacy).unwrap();
        assert_eq!(back.linecodes[0].i_max, Some(vec![f64::INFINITY, 400.0]));
    }

    /// A required field spelled `null` for a nonfinite value is still required:
    /// the schema must keep it in `required`, or it would accept a document the
    /// reader rejects for a missing key.
    #[cfg(feature = "schema")]
    #[test]
    fn a_nullable_scalar_stays_required_in_the_schema() {
        let schema = schemars::schema_for!(crate::model::DistCapacitor);
        let v = serde_json::to_value(&schema).unwrap();
        let required: Vec<&str> = v["required"]
            .as_array()
            .expect("required list")
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert!(required.contains(&"q_rated"), "required: {required:?}");
        assert!(required.contains(&"v_nom"), "required: {required:?}");
        assert_eq!(
            v["properties"]["q_rated"]["type"],
            serde_json::json!(["number", "null"])
        );
    }
}
