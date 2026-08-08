//! Payload spellings for nonfinite floats.
//!
//! serde_json writes a nonfinite `f64` as JSON `null`. These modules read
//! that `null` back, so a payload the library wrote always reads back
//! (#268). A `null` element in an upper bound means unbounded above
//! (+Inf); in a lower bound, unbounded below (-Inf); in a length, not
//! known (NaN). This is the PMD convention.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn restore(v: Option<Vec<Option<f64>>>, missing: f64) -> Option<Vec<f64>> {
    v.map(|xs| xs.into_iter().map(|x| x.unwrap_or(missing)).collect())
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
}
