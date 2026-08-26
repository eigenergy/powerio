//! Serde helpers that apply record limits while decoding.
//!
//! A hostile document must fail at the first excess element or byte, before an
//! unbounded `Vec`, map, or `String` has been built. Every helper here refuses
//! or truncates inside the visitor, so the only transient allocation is the
//! JSON scanner's own token buffer, which is bounded by the input size the
//! caller admitted.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserialize, DeserializeSeed, Deserializer, Error as _, SeqAccess, Visitor};
use serde_json::{Map, Value};

/// A string field that is refused past `max_bytes`, checked before the text is
/// retained.
pub(crate) struct BoundedStr {
    pub what: &'static str,
    pub max_bytes: usize,
}

impl Visitor<'_> for BoundedStr {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a {} of at most {} bytes",
            self.what, self.max_bytes
        )
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
        if value.len() > self.max_bytes {
            return Err(E::custom(format!(
                "a stored {} exceeds {} bytes",
                self.what, self.max_bytes
            )));
        }
        Ok(value.to_owned())
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<String, E> {
        if value.len() > self.max_bytes {
            return Err(E::custom(format!(
                "a stored {} exceeds {} bytes",
                self.what, self.max_bytes
            )));
        }
        Ok(value)
    }
}

impl<'de> DeserializeSeed<'de> for BoundedStr {
    type Value = String;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_str(self)
    }
}

/// A string field that is truncated at a character boundary once `max_bytes`
/// have been retained. Used for message text whose semantic limit is already a
/// truncation rule rather than a refusal.
pub(crate) struct TruncatedStr {
    pub max_bytes: usize,
}

impl Visitor<'_> for TruncatedStr {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a string retained up to {} bytes",
            self.max_bytes
        )
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
        if value.len() <= self.max_bytes {
            return Ok(value.to_owned());
        }
        let mut end = self.max_bytes;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        Ok(value[..end].to_owned())
    }

    fn visit_string<E: serde::de::Error>(self, mut value: String) -> Result<String, E> {
        if value.len() > self.max_bytes {
            let mut end = self.max_bytes;
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
        }
        Ok(value)
    }
}

/// A sequence field that is refused as soon as one element past `max_len`
/// arrives.
struct BoundedSeq<T> {
    what: &'static str,
    max_len: usize,
    marker: PhantomData<T>,
}

impl<'de, T: Deserialize<'de>> Visitor<'de> for BoundedSeq<T> {
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a sequence of at most {} {}",
            self.max_len, self.what
        )
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(self.max_len));
        while let Some(value) = seq.next_element::<T>()? {
            if values.len() == self.max_len {
                return Err(A::Error::custom(format!(
                    "a stored record carries more than {} {}",
                    self.max_len, self.what
                )));
            }
            values.push(value);
        }
        Ok(values)
    }
}

pub(crate) fn bounded_vec<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
    what: &'static str,
    max_len: usize,
) -> Result<Vec<T>, D::Error> {
    deserializer.deserialize_seq(BoundedSeq {
        what,
        max_len,
        marker: PhantomData,
    })
}

/// A JSON object field whose key count, key lengths, and key texts are
/// checked as each key arrives, before its value is read. The values remain
/// `serde_json::Value` and are bounded by the input size the caller admitted.
struct BoundedJsonMap {
    what: &'static str,
    max_keys: usize,
    max_key_bytes: usize,
    valid_key: fn(&str) -> bool,
}

impl<'de> Visitor<'de> for BoundedJsonMap {
    type Value = Map<String, Value>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "an object of at most {} {}",
            self.max_keys, self.what
        )
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut access: A,
    ) -> Result<Map<String, Value>, A::Error> {
        let mut map = Map::new();
        while let Some(key) = access.next_key_seed(BoundedStr {
            what: self.what,
            max_bytes: self.max_key_bytes,
        })? {
            if !(self.valid_key)(&key) {
                return Err(A::Error::custom(format!(
                    "a stored record carries an invalid {} key",
                    self.what
                )));
            }
            if map.len() == self.max_keys {
                return Err(A::Error::custom(format!(
                    "a stored record carries more than {} {}",
                    self.max_keys, self.what
                )));
            }
            map.insert(key, access.next_value()?);
        }
        Ok(map)
    }
}

pub(crate) fn bounded_json_map<'de, D: Deserializer<'de>>(
    deserializer: D,
    what: &'static str,
    max_keys: usize,
    max_key_bytes: usize,
    valid_key: fn(&str) -> bool,
) -> Result<Map<String, Value>, D::Error> {
    deserializer.deserialize_map(BoundedJsonMap {
        what,
        max_keys,
        max_key_bytes,
        valid_key,
    })
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeSeed;
    use serde::de::IntoDeserializer;

    use super::*;

    fn json_de(text: &str) -> serde_json::Deserializer<serde_json::de::StrRead<'_>> {
        serde_json::Deserializer::from_str(text)
    }

    #[test]
    fn oversized_strings_are_refused_before_retention() {
        let seed = BoundedStr {
            what: "identifier",
            max_bytes: 4,
        };
        assert_eq!(
            seed.deserialize(String::from("abcd").into_deserializer()
                as serde::de::value::StringDeserializer<serde_json::Error>)
                .unwrap(),
            "abcd"
        );
        let seed = BoundedStr {
            what: "identifier",
            max_bytes: 4,
        };
        assert!(
            seed.deserialize(String::from("abcde").into_deserializer()
                as serde::de::value::StringDeserializer<serde_json::Error>)
                .is_err()
        );
    }

    #[test]
    fn truncated_strings_stop_at_a_character_boundary() {
        let visitor = TruncatedStr { max_bytes: 4 };
        let text = "aééé";
        let kept = visitor.visit_str::<serde_json::Error>(text).unwrap();
        assert_eq!(kept, "aé");
        assert!(kept.len() <= 4);
    }

    #[test]
    fn a_sequence_fails_at_the_first_excess_element() {
        let mut deserializer = json_de("[1,2,3,4]");
        let result: Result<Vec<u32>, _> = bounded_vec(&mut deserializer, "entries", 3);
        assert!(result.unwrap_err().to_string().contains("more than 3"));

        let mut deserializer = json_de("[1,2,3]");
        let values: Vec<u32> = bounded_vec(&mut deserializer, "entries", 3).unwrap();
        assert_eq!(values, [1, 2, 3]);
    }

    #[test]
    fn a_map_checks_key_count_length_and_text_while_decoding() {
        let accept = |_: &str| true;
        let mut deserializer = json_de(r#"{"a":1,"b":2}"#);
        let map = bounded_json_map(&mut deserializer, "detail keys", 2, 8, accept).unwrap();
        assert_eq!(map.len(), 2);

        let mut deserializer = json_de(r#"{"a":1,"b":2,"c":3}"#);
        assert!(bounded_json_map(&mut deserializer, "detail keys", 2, 8, accept).is_err());

        let mut deserializer = json_de(r#"{"toolong":1}"#);
        assert!(bounded_json_map(&mut deserializer, "detail keys", 8, 4, accept).is_err());

        // The key predicate runs as the key is decoded, before its value.
        let mut deserializer = json_de(r#"{"":1}"#);
        assert!(
            bounded_json_map(&mut deserializer, "detail keys", 8, 8, |key| !key
                .is_empty())
            .is_err()
        );
    }
}
