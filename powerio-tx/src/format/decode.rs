//! Shared typed-decode helpers for the JSON readers: lenient per-field
//! deserializers matching the old tree walk's `Value::as_*` coercions, and a
//! table collector that keeps document order without an intermediate tree.

use serde_json::Value;

/// A keyed table object collected in document order without an intermediate
/// tree. A section that is not an object reads as empty, the same leniency
/// the tree walk gave it.
pub(crate) fn lenient_table<'de, D, T>(d: D) -> std::result::Result<Vec<(String, T)>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct TableVisitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for TableVisitor<T> {
        type Value = Vec<(String, T)>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a table object")
        }

        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut access: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut rows = Vec::new();
            while let Some((key, row)) = access.next_entry::<String, T>()? {
                rows.push((key, row));
            }
            Ok(rows)
        }

        fn visit_seq<S: serde::de::SeqAccess<'de>>(
            self,
            mut access: S,
        ) -> std::result::Result<Self::Value, S::Error> {
            while access.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok(Vec::new())
        }

        fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_bool<E>(self, _: bool) -> std::result::Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_i64<E>(self, _: i64) -> std::result::Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_u64<E>(self, _: u64) -> std::result::Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_f64<E>(self, _: f64) -> std::result::Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_str<E>(self, _: &str) -> std::result::Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }
    d.deserialize_any(TableVisitor(std::marker::PhantomData))
}

/// Elements of one section ordered by their inner `index` so a re-emitted
/// file assigns the same running keys.
pub(crate) fn sorted_rows<T>(
    mut rows: Vec<(String, T)>,
    index: impl Fn(&T) -> Option<i64>,
) -> Vec<(String, T)> {
    rows.sort_by_key(|(_, row)| index(row).unwrap_or(0));
    rows
}

/// Absent or null reads `None`; a present value of the wrong type also reads
/// `None`, the same leniency `Value::as_f64` gave the tree walk.
pub(crate) fn lenient_f64<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<f64>, D::Error> {
    use serde::Deserialize;
    Ok(Option::<Value>::deserialize(d)?
        .as_ref()
        .and_then(Value::as_f64))
}

pub(crate) fn lenient_u64<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<u64>, D::Error> {
    use serde::Deserialize;
    Ok(Option::<Value>::deserialize(d)?
        .as_ref()
        .and_then(Value::as_u64))
}

pub(crate) fn lenient_i64<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<i64>, D::Error> {
    use serde::Deserialize;
    Ok(Option::<Value>::deserialize(d)?
        .as_ref()
        .and_then(Value::as_i64))
}

pub(crate) fn lenient_bool<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<bool>, D::Error> {
    use serde::Deserialize;
    Ok(Option::<Value>::deserialize(d)?
        .as_ref()
        .and_then(Value::as_bool))
}

pub(crate) fn lenient_string<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    use serde::Deserialize;
    Ok(Option::<Value>::deserialize(d)?
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// A 0/1 status field. Some producers write a JSON boolean instead of the
/// MATPOWER 0/1 number, and any other nonzero-or-non-numeric value reads in
/// service — the exact leniency the tree walk gave `flag`.
pub(crate) fn lenient_flag<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<bool>, D::Error> {
    use serde::Deserialize;
    Ok(Option::<Value>::deserialize(d)?.map(|value| match value {
        Value::Bool(b) => b,
        other => other.as_f64() != Some(0.0),
    }))
}
