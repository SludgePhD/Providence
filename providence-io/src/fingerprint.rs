use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use serde::{
    de::{value::Error, EnumAccess, Error as _, SeqAccess, VariantAccess, Visitor},
    Deserialize, Deserializer,
};

/// Computes a fingerprint for a deserializable type that changes whenever the type's structure changes.
///
/// This allows detecting when a type's serialization has changed, for example to detect version
/// mismatches.
pub fn serde_fingerprint<'de, S: Deserialize<'de>>() -> u64 {
    let mut hasher = DefaultHasher::new();
    S::deserialize(Deser {
        hasher: &mut hasher,
    })
    .unwrap();
    hasher.finish()
}

struct Seq<'a> {
    hasher: &'a mut DefaultHasher,
    len: usize,
}

impl<'a, 'de> SeqAccess<'de> for Seq<'a> {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        if self.len == 0 {
            return Ok(None);
        }

        self.len -= 1;
        seed.deserialize(Deser {
            hasher: self.hasher,
        })
        .map(Some)
    }
}

#[allow(dead_code)]
struct Enum<'a> {
    hasher: &'a mut DefaultHasher,
    len: usize,
}

impl<'a, 'de> EnumAccess<'de> for Enum<'a> {
    type Error = Error;

    type Variant = Variant<'a>;

    fn variant_seed<V>(self, _seed: V) -> Result<(V::Value, Self::Variant), Self::Error>
    where
        V: serde::de::DeserializeSeed<'de>,
    {
        Err(Error::custom("enum fingerprinting is not yet supported"))
    }
}

struct Variant<'a> {
    hasher: &'a mut DefaultHasher,
}

impl<'a, 'de> VariantAccess<'de> for Variant<'a> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        self.hasher.write(b"unit_variant");
        Ok(())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        self.hasher.write(b"newtype_variant_seed");
        seed.deserialize(Deser {
            hasher: self.hasher,
        })
    }

    fn tuple_variant<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"tuple_variant");
        self.hasher.write_usize(len);
        visitor.visit_seq(Seq {
            hasher: self.hasher,
            len,
        })
    }

    fn struct_variant<V>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"struct_variant");
        fields.hash(self.hasher);
        visitor.visit_seq(Seq {
            hasher: self.hasher,
            len: fields.len(),
        })
    }
}

struct Deser<'a> {
    hasher: &'a mut DefaultHasher,
}

impl<'a, 'de> Deserializer<'de> for Deser<'a> {
    type Error = Error;

    fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::custom("`deserialize_any` is not supported"))
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"bool");
        visitor.visit_bool(false)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"i8");
        visitor.visit_i8(0)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"i16");
        visitor.visit_i16(0)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"i32");
        visitor.visit_i32(0)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"i64");
        visitor.visit_i64(0)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"u8");
        visitor.visit_u8(0)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"u16");
        visitor.visit_u16(0)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"u32");
        visitor.visit_u32(0)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"u64");
        visitor.visit_u64(0)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"f32");
        visitor.visit_f32(0.0)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"f64");
        visitor.visit_f64(0.0)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"char");
        visitor.visit_char('c')
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"str");
        visitor.visit_borrowed_str("borrowed_str")
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"str");
        visitor.visit_str("string")
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"bytes");
        visitor.visit_bytes(&[])
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"bytes");
        visitor.visit_bytes(&[])
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"option");
        visitor.visit_none()
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"unit");
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"unit_struct");
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"newtype_struct");
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"seq");
        visitor.visit_seq(Seq {
            hasher: self.hasher,
            len: 0,
        })
    }

    fn deserialize_tuple<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"tuple");
        self.hasher.write_usize(len);
        visitor.visit_seq(Seq {
            hasher: self.hasher,
            len,
        })
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"tuple_struct");
        self.hasher.write_usize(len);
        visitor.visit_seq(Seq {
            hasher: self.hasher,
            len,
        })
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"map");
        visitor.visit_seq(Seq {
            hasher: self.hasher,
            len: 0,
        })
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"struct");
        fields.hash(self.hasher);
        visitor.visit_seq(Seq {
            hasher: self.hasher,
            len: fields.len(),
        })
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"enum");
        variants.hash(self.hasher);
        visitor.visit_enum(Enum {
            hasher: self.hasher,
            len: variants.len(),
        })
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.hasher.write(b"identifier");
        visitor.visit_u32(0) // FIXME unsure what this is for??
    }

    fn deserialize_ignored_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::custom("`deserialize_ignored_any` is not supported"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same<'de, T: Deserialize<'de>, U: Deserialize<'de>>() {
        let f1 = serde_fingerprint::<T>();
        let f2 = serde_fingerprint::<U>();
        assert_eq!(f1, f2);
    }

    fn different<'de, T: Deserialize<'de>, U: Deserialize<'de>>() {
        let f1 = serde_fingerprint::<T>();
        let f2 = serde_fingerprint::<U>();
        assert_ne!(f1, f2);
    }

    #[test]
    fn field_name_change() {
        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct S1 {
            a: u8,
            b: u32,
        }

        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct S2 {
            a: u8,
            c: u32,
        }

        different::<S1, S2>();
    }

    #[test]
    fn string_ownership() {
        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct S1 {
            a: u8,
            b: u32,
            c: String,
        }

        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct S2<'a> {
            a: u8,
            b: u32,
            c: &'a str,
        }

        same::<S1, S2<'static>>();
    }

    #[test]
    fn struct_int_change() {
        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct S<T> {
            a: u8,
            b: T,
            c: u64,
        }

        different::<S<u8>, S<u16>>();
        different::<S<u8>, S<i8>>();
        same::<S<u8>, S<u8>>();
    }
}
