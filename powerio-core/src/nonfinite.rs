//! JSON spellings for nonfinite floats, one convention for every document
//! powerio authors.
//!
//! JSON has no `Inf`/`NaN` literal. A nonfinite `f64` is written as one of
//! three strings — `"Infinity"`, `"-Infinity"`, `"NaN"` — and a float
//! position reads back either a number or one of those spellings, so every
//! value the library holds round trips through model JSON and the
//! `.pio.json` document. Readers legitimately produce nonfinite values (an
//! absent reactive limit is `Inf` in MATPOWER, PowerModels, pandapower, and
//! PyPSA), so the transport must carry them or refuse real cases. The
//! The mechanism is a pair of forwarding wrappers threaded through serde:
//! [`NonFiniteSer`] intercepts `serialize_f64`/`serialize_f32`, and
//! [`NonFiniteDe`] intercepts `deserialize_f64`/`deserialize_f32`, each
//! forwarding everything else — including `extras` maps, whose string values
//! pass through untouched because interception is keyed on the *type* being
//! an `f64`, never on a string's content. `BalancedNetwork`'s `Serialize`
//! and `Deserialize` impls route through the wrappers, so the spelling holds
//! on every serialization route (model JSON, the `.pio.json` payload, the C
//! ABI), including serde's internal buffering for tagged enums. Interception
//! applies only to human readable formats: a binary serializer receives the
//! plain `f64`.

use core::fmt;

use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde::{Deserializer, Serialize, Serializer};

/// The spelling of `f64::INFINITY` in model JSON.
pub const INFINITY: &str = "Infinity";
/// The spelling of `f64::NEG_INFINITY` in model JSON.
pub const NEG_INFINITY: &str = "-Infinity";
/// The spelling of `f64::NAN` in model JSON.
pub const NAN: &str = "NaN";

fn spell(v: f64) -> &'static str {
    if v.is_nan() {
        NAN
    } else if v > 0.0 {
        INFINITY
    } else {
        NEG_INFINITY
    }
}

fn unspell(s: &str) -> Option<f64> {
    match s {
        INFINITY => Some(f64::INFINITY),
        NEG_INFINITY => Some(f64::NEG_INFINITY),
        NAN => Some(f64::NAN),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Serialization: forward everything, spell a nonfinite float as a string.
// ---------------------------------------------------------------------------

/// Wraps any [`Serializer`], writing nonfinite floats as their string
/// spellings on human readable formats and forwarding everything else.
pub struct NonFiniteSer<S>(pub S);

/// Re-enters the wrapper for a nested value, so the interception threads
/// through sequences, maps, and struct fields.
struct Adapt<'a, T: ?Sized>(&'a T);

impl<T: Serialize + ?Sized> Serialize for Adapt<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(NonFiniteSer(serializer))
    }
}

impl<S: Serializer> Serializer for NonFiniteSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = SeqSer<S::SerializeSeq>;
    type SerializeTuple = TupleSer<S::SerializeTuple>;
    type SerializeTupleStruct = TupleStructSer<S::SerializeTupleStruct>;
    type SerializeTupleVariant = TupleVariantSer<S::SerializeTupleVariant>;
    type SerializeMap = MapSer<S::SerializeMap>;
    type SerializeStruct = StructSer<S::SerializeStruct>;
    type SerializeStructVariant = StructVariantSer<S::SerializeStructVariant>;

    fn serialize_f32(self, v: f32) -> Result<S::Ok, S::Error> {
        if v.is_finite() || !self.0.is_human_readable() {
            self.0.serialize_f32(v)
        } else {
            self.0.serialize_str(spell(f64::from(v)))
        }
    }

    fn serialize_f64(self, v: f64) -> Result<S::Ok, S::Error> {
        if v.is_finite() || !self.0.is_human_readable() {
            self.0.serialize_f64(v)
        } else {
            self.0.serialize_str(spell(v))
        }
    }

    fn serialize_bool(self, v: bool) -> Result<S::Ok, S::Error> {
        self.0.serialize_bool(v)
    }
    fn serialize_i8(self, v: i8) -> Result<S::Ok, S::Error> {
        self.0.serialize_i8(v)
    }
    fn serialize_i16(self, v: i16) -> Result<S::Ok, S::Error> {
        self.0.serialize_i16(v)
    }
    fn serialize_i32(self, v: i32) -> Result<S::Ok, S::Error> {
        self.0.serialize_i32(v)
    }
    fn serialize_i64(self, v: i64) -> Result<S::Ok, S::Error> {
        self.0.serialize_i64(v)
    }
    fn serialize_i128(self, v: i128) -> Result<S::Ok, S::Error> {
        self.0.serialize_i128(v)
    }
    fn serialize_u8(self, v: u8) -> Result<S::Ok, S::Error> {
        self.0.serialize_u8(v)
    }
    fn serialize_u16(self, v: u16) -> Result<S::Ok, S::Error> {
        self.0.serialize_u16(v)
    }
    fn serialize_u32(self, v: u32) -> Result<S::Ok, S::Error> {
        self.0.serialize_u32(v)
    }
    fn serialize_u64(self, v: u64) -> Result<S::Ok, S::Error> {
        self.0.serialize_u64(v)
    }
    fn serialize_u128(self, v: u128) -> Result<S::Ok, S::Error> {
        self.0.serialize_u128(v)
    }
    fn serialize_char(self, v: char) -> Result<S::Ok, S::Error> {
        self.0.serialize_char(v)
    }
    fn serialize_str(self, v: &str) -> Result<S::Ok, S::Error> {
        self.0.serialize_str(v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<S::Ok, S::Error> {
        self.0.serialize_bytes(v)
    }
    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_none()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_some(&Adapt(value))
    }
    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit()
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit_struct(name)
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit_variant(name, variant_index, variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_newtype_struct(name, &Adapt(value))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.0
            .serialize_newtype_variant(name, variant_index, variant, &Adapt(value))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        self.0.serialize_seq(len).map(SeqSer)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        self.0.serialize_tuple(len).map(TupleSer)
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, S::Error> {
        self.0.serialize_tuple_struct(name, len).map(TupleStructSer)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        self.0
            .serialize_tuple_variant(name, variant_index, variant, len)
            .map(TupleVariantSer)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        self.0.serialize_map(len).map(MapSer)
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, S::Error> {
        self.0.serialize_struct(name, len).map(StructSer)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        self.0
            .serialize_struct_variant(name, variant_index, variant, len)
            .map(StructVariantSer)
    }
    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<S::Ok, S::Error> {
        self.0.collect_str(value)
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

#[doc(hidden)]
pub struct SeqSer<S>(S);
impl<S: SerializeSeq> SerializeSeq for SeqSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_element(&Adapt(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

#[doc(hidden)]
pub struct TupleSer<S>(S);
impl<S: SerializeTuple> SerializeTuple for TupleSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_element(&Adapt(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

#[doc(hidden)]
pub struct TupleStructSer<S>(S);
impl<S: SerializeTupleStruct> SerializeTupleStruct for TupleStructSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_field(&Adapt(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

#[doc(hidden)]
pub struct TupleVariantSer<S>(S);
impl<S: SerializeTupleVariant> SerializeTupleVariant for TupleVariantSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_field(&Adapt(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

#[doc(hidden)]
pub struct MapSer<S>(S);
impl<S: SerializeMap> SerializeMap for MapSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    // Keys forward unwrapped: a nonfinite float key stays the error it is
    // today rather than silently becoming a string key.
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), S::Error> {
        self.0.serialize_key(key)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_value(&Adapt(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

#[doc(hidden)]
pub struct StructSer<S>(S);
impl<S: SerializeStruct> SerializeStruct for StructSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), S::Error> {
        self.0.serialize_field(key, &Adapt(value))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

#[doc(hidden)]
pub struct StructVariantSer<S>(S);
impl<S: SerializeStructVariant> SerializeStructVariant for StructVariantSer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), S::Error> {
        self.0.serialize_field(key, &Adapt(value))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

// ---------------------------------------------------------------------------
// Deserialization: forward everything, read the string spellings back at
// float positions.
// ---------------------------------------------------------------------------

/// Wraps any [`Deserializer`], accepting the string spellings wherever an
/// `f64`/`f32` is expected on human readable formats and forwarding
/// everything else.
pub struct NonFiniteDe<D>(pub D);

macro_rules! forward_deserialize {
    ($($method:ident)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
                self.0.$method(Wrap(visitor))
            }
        )*
    };
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for NonFiniteDe<D> {
    type Error = D::Error;

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        if self.0.is_human_readable() {
            self.0.deserialize_any(NumOrSpelling(visitor))
        } else {
            self.0.deserialize_f32(visitor)
        }
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        if self.0.is_human_readable() {
            self.0.deserialize_any(NumOrSpelling(visitor))
        } else {
            self.0.deserialize_f64(visitor)
        }
    }

    forward_deserialize! {
        deserialize_any deserialize_bool
        deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
        deserialize_char deserialize_str deserialize_string
        deserialize_bytes deserialize_byte_buf
        deserialize_option deserialize_unit
        deserialize_seq deserialize_map deserialize_identifier deserialize_ignored_any
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.0.deserialize_unit_struct(name, Wrap(visitor))
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.0.deserialize_newtype_struct(name, Wrap(visitor))
    }
    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.0.deserialize_tuple(len, Wrap(visitor))
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.0.deserialize_tuple_struct(name, len, Wrap(visitor))
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.0.deserialize_struct(name, fields, Wrap(visitor))
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.0.deserialize_enum(name, variants, Wrap(visitor))
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

/// At a float position, numbers forward and the three spellings convert.
struct NumOrSpelling<V>(V);

impl<'de, V: Visitor<'de>> Visitor<'de> for NumOrSpelling<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a number or one of \"Infinity\", \"-Infinity\", \"NaN\"")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<V::Value, E> {
        self.0.visit_i64(v)
    }
    fn visit_i128<E: de::Error>(self, v: i128) -> Result<V::Value, E> {
        self.0.visit_i128(v)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<V::Value, E> {
        self.0.visit_u64(v)
    }
    fn visit_u128<E: de::Error>(self, v: u128) -> Result<V::Value, E> {
        self.0.visit_u128(v)
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<V::Value, E> {
        self.0.visit_f64(v)
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<V::Value, E> {
        match unspell(s) {
            Some(v) => self.0.visit_f64(v),
            None => Err(de::Error::invalid_value(de::Unexpected::Str(s), &self)),
        }
    }

    fn visit_unit<E: de::Error>(self) -> Result<V::Value, E> {
        Err(de::Error::custom(
            "a floating point value cannot be null; use a number, \"Infinity\", \
             \"-Infinity\", or \"NaN\"",
        ))
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<V::Value, E> {
        self.0.visit_bool(v)
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<V::Value, A::Error> {
        self.0.visit_map(map)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<V::Value, A::Error> {
        self.0.visit_seq(seq)
    }
}

/// Generic forwarding visitor: threads the wrapper into nested access types
/// so a float anywhere in the tree is intercepted, and forwards every scalar
/// verbatim — a string only converts at a float position, never here.
struct Wrap<V>(V);

impl<'de, V: Visitor<'de>> Visitor<'de> for Wrap<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.expecting(f)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<V::Value, E> {
        self.0.visit_bool(v)
    }
    fn visit_i8<E: de::Error>(self, v: i8) -> Result<V::Value, E> {
        self.0.visit_i8(v)
    }
    fn visit_i16<E: de::Error>(self, v: i16) -> Result<V::Value, E> {
        self.0.visit_i16(v)
    }
    fn visit_i32<E: de::Error>(self, v: i32) -> Result<V::Value, E> {
        self.0.visit_i32(v)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<V::Value, E> {
        self.0.visit_i64(v)
    }
    fn visit_i128<E: de::Error>(self, v: i128) -> Result<V::Value, E> {
        self.0.visit_i128(v)
    }
    fn visit_u8<E: de::Error>(self, v: u8) -> Result<V::Value, E> {
        self.0.visit_u8(v)
    }
    fn visit_u16<E: de::Error>(self, v: u16) -> Result<V::Value, E> {
        self.0.visit_u16(v)
    }
    fn visit_u32<E: de::Error>(self, v: u32) -> Result<V::Value, E> {
        self.0.visit_u32(v)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<V::Value, E> {
        self.0.visit_u64(v)
    }
    fn visit_u128<E: de::Error>(self, v: u128) -> Result<V::Value, E> {
        self.0.visit_u128(v)
    }
    fn visit_f32<E: de::Error>(self, v: f32) -> Result<V::Value, E> {
        self.0.visit_f32(v)
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<V::Value, E> {
        self.0.visit_f64(v)
    }
    fn visit_char<E: de::Error>(self, v: char) -> Result<V::Value, E> {
        self.0.visit_char(v)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<V::Value, E> {
        self.0.visit_str(v)
    }
    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<V::Value, E> {
        self.0.visit_borrowed_str(v)
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<V::Value, E> {
        self.0.visit_string(v)
    }
    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<V::Value, E> {
        self.0.visit_bytes(v)
    }
    fn visit_borrowed_bytes<E: de::Error>(self, v: &'de [u8]) -> Result<V::Value, E> {
        self.0.visit_borrowed_bytes(v)
    }
    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<V::Value, E> {
        self.0.visit_byte_buf(v)
    }
    fn visit_none<E: de::Error>(self) -> Result<V::Value, E> {
        self.0.visit_none()
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<V::Value, D::Error> {
        self.0.visit_some(NonFiniteDe(deserializer))
    }
    fn visit_unit<E: de::Error>(self) -> Result<V::Value, E> {
        self.0.visit_unit()
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<V::Value, D::Error> {
        self.0.visit_newtype_struct(NonFiniteDe(deserializer))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<V::Value, A::Error> {
        self.0.visit_seq(SeqDe(seq))
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<V::Value, A::Error> {
        self.0.visit_map(MapDe(map))
    }
    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<V::Value, A::Error> {
        self.0.visit_enum(EnumDe(data))
    }
}

struct SeqDe<A>(A);
impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for SeqDe<A> {
    type Error = A::Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, A::Error> {
        self.0.next_element_seed(SeedDe(seed))
    }
    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

struct MapDe<A>(A);
impl<'de, A: MapAccess<'de>> MapAccess<'de> for MapDe<A> {
    type Error = A::Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        self.0.next_key_seed(SeedDe(seed))
    }
    fn next_value_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<T::Value, A::Error> {
        self.0.next_value_seed(SeedDe(seed))
    }
    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

struct EnumDe<A>(A);
impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for EnumDe<A> {
    type Error = A::Error;
    type Variant = VariantDe<A::Variant>;
    fn variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<(T::Value, Self::Variant), A::Error> {
        self.0
            .variant_seed(SeedDe(seed))
            .map(|(value, variant)| (value, VariantDe(variant)))
    }
}

struct VariantDe<A>(A);
impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for VariantDe<A> {
    type Error = A::Error;
    fn unit_variant(self) -> Result<(), A::Error> {
        self.0.unit_variant()
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        self.0.newtype_variant_seed(SeedDe(seed))
    }
    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        self.0.tuple_variant(len, Wrap(visitor))
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.0.struct_variant(fields, Wrap(visitor))
    }
}

struct SeedDe<S>(S);
impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for SeedDe<S> {
    type Value = S::Value;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        self.0.deserialize(NonFiniteDe(deserializer))
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Probe {
        qmax: f64,
        qmin: f64,
        opt: Option<f64>,
        list: Vec<f64>,
        name: String,
        extras: serde_json::Value,
    }

    fn to_json(p: &Probe) -> String {
        let mut out = Vec::new();
        let mut ser = serde_json::Serializer::new(&mut out);
        Serialize::serialize(p, super::NonFiniteSer(&mut ser)).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn from_json(text: &str) -> Result<Probe, serde_json::Error> {
        let mut de = serde_json::Deserializer::from_str(text);
        Deserialize::deserialize(super::NonFiniteDe(&mut de))
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn nonfinite_floats_spell_as_strings_and_read_back() {
        let p = Probe {
            qmax: f64::INFINITY,
            qmin: f64::NEG_INFINITY,
            opt: Some(f64::NAN),
            list: vec![1.5, f64::INFINITY],
            name: "g1".into(),
            extras: serde_json::json!({"note": "Infinity"}),
        };
        let text = to_json(&p);
        assert!(text.contains("\"qmax\":\"Infinity\""), "{text}");
        assert!(text.contains("\"qmin\":\"-Infinity\""), "{text}");
        assert!(text.contains("\"opt\":\"NaN\""), "{text}");
        assert!(text.contains("[1.5,\"Infinity\"]"), "{text}");
        // The extras string is untouched: interception is by type, never by
        // a string's content.
        assert!(text.contains("\"note\":\"Infinity\""), "{text}");

        let back = from_json(&text).unwrap();
        assert_eq!(back.qmax, f64::INFINITY);
        assert_eq!(back.qmin, f64::NEG_INFINITY);
        assert!(back.opt.unwrap().is_nan());
        assert_eq!(back.list[1], f64::INFINITY);
        assert_eq!(back.extras["note"], "Infinity");

        // Second write is byte stable.
        assert_eq!(to_json(&back), text);
    }

    #[test]
    fn finite_values_serialize_as_numbers() {
        let p = Probe {
            qmax: 9900.0,
            qmin: -9900.0,
            opt: None,
            list: vec![],
            name: String::new(),
            extras: serde_json::Value::Null,
        };
        let text = to_json(&p);
        assert!(text.contains("\"qmax\":9900.0"), "{text}");
        assert!(!text.contains("Infinity"), "{text}");
    }

    #[test]
    fn an_unknown_string_at_a_float_position_is_refused() {
        let err = from_json(
            r#"{"qmax":"unbounded","qmin":0.0,"opt":null,"list":[],"name":"","extras":null}"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Infinity"), "{msg}");
        assert!(msg.contains("unbounded"), "{msg}");
    }

    #[test]
    fn a_null_at_a_float_position_names_the_accepted_values() {
        let err =
            from_json(r#"{"qmax":null,"qmin":0.0,"opt":null,"list":[],"name":"","extras":null}"#)
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cannot be null"), "{msg}");
        assert!(msg.contains("\"Infinity\""), "{msg}");
        assert!(msg.contains("\"-Infinity\""), "{msg}");
        assert!(msg.contains("\"NaN\""), "{msg}");
    }
}
