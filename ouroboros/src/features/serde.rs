pub mod ser {
    use serde::{
        ser::{SerializeMap as _, SerializeSeq},
        Serialize, Serializer,
    };

    use crate::{
        field::Fields,
        product::{Array, Record, Tuple},
        sum::{Enum, EnumVariant, Optional, Union, UnionVariant},
        symbolic::Symbolic,
        type_info::Type,
        Alias, Fallible, Func, Generic, NamedFields, Ptr, RecordDocs, RecordFieldDocs,
        UnnamedFields,
    };

    impl Serialize for Type {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match self {
                // Basic types
                Self::Unit => serializer.serialize_str("()"),
                Self::Bool => serializer.serialize_str("bool"),
                Self::U8 => serializer.serialize_str("u8"),
                Self::U16 => serializer.serialize_str("u16"),
                Self::U32 => serializer.serialize_str("u32"),
                Self::U64 => serializer.serialize_str("u64"),
                Self::U128 => serializer.serialize_str("u128"),
                Self::I8 => serializer.serialize_str("i8"),
                Self::I16 => serializer.serialize_str("i16"),
                Self::I32 => serializer.serialize_str("i32"),
                Self::I64 => serializer.serialize_str("i64"),
                Self::I128 => serializer.serialize_str("i128"),
                Self::F32 => serializer.serialize_str("f32"),
                Self::F64 => serializer.serialize_str("f64"),
                Self::String => serializer.serialize_str("string"),

                // Product types
                Self::Array(array) => array.serialize(serializer),
                Self::Func(func) => func.serialize(serializer),
                Self::Record(fields) => fields.serialize(serializer),
                Self::Tuple(tuple) => tuple.serialize(serializer),

                // Sum types
                Self::Enum(e) => e.serialize(serializer),
                Self::Fallible(fall) => fall.serialize(serializer),
                Self::Optional(optional) => optional.serialize(serializer),
                Self::Union(union) => union.serialize(serializer),

                // Special types
                Self::Ptr(p) => p.serialize(serializer),
                Self::Symbolic(sym) => sym.serialize(serializer),
                Self::Generic(gen) => gen.serialize(serializer),
                Self::Alias(alias) => alias.serialize(serializer),
            }
        }
    }

    //
    // Product types
    //

    impl Serialize for Array {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", "array")?;
            map.serialize_entry("t", &self.t)?;
            map.end()
        }
    }

    impl Serialize for Func {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", "λ")?;
            map.serialize_entry("t", &[&self.a, &self.b])?;
            map.end()
        }
    }

    impl Serialize for RecordDocs {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            if let Some(record) = &self.record {
                map.serialize_entry("record", record)?;
            }
            if let Some(fields) = &self.fields {
                map.serialize_entry("fields", fields)?;
            }
            map.end()
        }
    }

    impl Serialize for RecordFieldDocs {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match self {
                Self::Named(fields) => {
                    let mut map = serializer.serialize_map(Some(fields.len()))?;
                    for (f, doc) in fields.iter() {
                        map.serialize_entry(&f, &doc)?;
                    }
                    map.end()
                }
                Self::Unnamed(fields) => {
                    let mut seq = serializer.serialize_seq(Some(fields.len()))?;
                    for doc in fields.iter() {
                        seq.serialize_element(&doc)?;
                    }
                    seq.end()
                }
            }
        }
    }

    impl Serialize for Record {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(4))?;
            if let Some(doc) = &self.doc {
                map.serialize_entry("doc", doc)?;
            }
            map.serialize_entry("k", "record")?;
            map.serialize_entry("t", &self.fields)?;
            map.serialize_entry("n", &self.n)?;
            map.end()
        }
    }

    impl Serialize for Tuple {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", "tuple")?;
            map.serialize_entry("t", &self.fields)?;
            map.end()
        }
    }

    //
    // Sum types
    //

    impl Serialize for Enum {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(4))?;
            if let Some(d) = &self.doc {
                map.serialize_entry("doc", d)?;
            }
            map.serialize_entry("k", "enum")?;
            map.serialize_entry("t", &self.variants)?;
            map.serialize_entry("n", &self.n)?;
            map.end()
        }
    }

    impl Serialize for EnumVariant {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match &self.v {
                Some(v) => {
                    let mut map = serializer.serialize_map(Some(1))?;
                    map.serialize_entry(&self.n, &v)?;
                    map.end()
                }
                None => serializer.serialize_str(&self.n),
            }
        }
    }

    impl Serialize for Fallible {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", "fallible")?;
            map.serialize_entry("t", &[&self.ok, &self.err])?;
            map.end()
        }
    }

    impl Serialize for Optional {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", "optional")?;
            map.serialize_entry("t", &self.t)?;
            map.end()
        }
    }

    impl Serialize for Union {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(4))?;
            if let Some(d) = &self.doc {
                map.serialize_entry("doc", d)?;
            }
            map.serialize_entry("k", "union")?;
            map.serialize_entry("t", &self.variants)?;
            map.serialize_entry("n", &self.n)?;
            map.end()
        }
    }

    impl Serialize for UnionVariant {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match &self.fields {
                Some(fields) => {
                    let mut map = serializer.serialize_map(Some(1))?;
                    map.serialize_entry(&self.n, &fields)?;
                    map.end()
                }
                None => serializer.serialize_str(&self.n),
            }
        }
    }

    //
    // Special types
    //

    impl Serialize for Ptr {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", "@")?;
            map.serialize_entry("t", &self.t)?;
            map.end()
        }
    }

    impl Serialize for Symbolic {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&format!("${}", &self.n))
        }
    }

    impl Serialize for Generic {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&format!("^{}", &self.n))
        }
    }

    impl Serialize for Alias {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("k", "alias")?;
            map.serialize_entry("t", &self.t)?;
            map.end()
        }
    }

    //
    // Fields
    //

    impl Serialize for Fields {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match self {
                Fields::Named(fields) => fields.serialize(serializer),
                Fields::Unnamed(fields) => fields.serialize(serializer),
            }
        }
    }

    impl Serialize for NamedFields {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(self.fields.len()))?;
            for field in self.fields.iter() {
                map.serialize_entry(&field.n, &field.t)?;
            }
            map.end()
        }
    }

    impl Serialize for UnnamedFields {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut seq = serializer.serialize_seq(Some(self.fields.len()))?;
            for field in self.fields.iter() {
                seq.serialize_element(&field.t)?;
            }
            seq.end()
        }
    }
}

pub mod de {
    use std::{
        collections::HashMap,
        fmt::{self, Formatter},
    };

    use serde::{
        de::{self, MapAccess, SeqAccess, Visitor},
        Deserialize, Deserializer,
    };

    use crate::{
        field::{Fields, NamedField, UnnamedField},
        product::{Array, Record, Tuple},
        sum::{Enum, EnumVariant, Optional, Union, UnionVariant},
        symbolic::Symbolic,
        type_info::Type,
        Alias, Fallible, Func, Generic, Ptr, RecordDocs, RecordFieldDocs,
    };

    /// Suspended types are types that are not yet fully deserialized. While
    /// their structure is known, the associated kind is not known, and so they
    /// cannot be interpreted.
    #[derive(Clone)]
    enum SuspendedType {
        Null,
        U64(u64),
        Str(String),
        Seq(Vec<SuspendedType>),
        Map(HashMap<String, SuspendedType>),
    }

    impl<E> From<SuspendedType> for Result<RecordDocs, E>
    where
        E: de::Error,
    {
        fn from(suspended_type: SuspendedType) -> Self {
            match suspended_type {
                // Documentation is expected to be a map
                SuspendedType::Map(m) => {
                    let record = if let Some(rec) = m.get("record") {
                        if let SuspendedType::Str(s) = rec {
                            Some(s.clone())
                        } else {
                            return Err(E::custom("invalid record docs"));
                        }
                    } else {
                        None
                    };
                    let fields = if let Some(fields) = m.get("fields") {
                        match fields {
                            SuspendedType::Map(m) => {
                                let fields = m
                                    .iter()
                                    .map(|(n, t)| {
                                        if let SuspendedType::Str(s) = t {
                                            Ok((n.clone(), s.clone()))
                                        } else {
                                            Err(E::custom("invalid record field docs"))
                                        }
                                    })
                                    .collect::<Result<HashMap<_, _>, _>>()?;
                                Some(RecordFieldDocs::Named(fields))
                            }
                            SuspendedType::Seq(seq) => {
                                let fields = seq
                                    .iter()
                                    .map(|t| {
                                        if let SuspendedType::Str(s) = t {
                                            Ok(Some(s.clone()))
                                        } else {
                                            Err(E::custom("invalid record field docs"))
                                        }
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                Some(RecordFieldDocs::Unnamed(fields))
                            }
                            _ => {
                                return Err(E::custom("invalid record field docs"));
                            }
                        }
                    } else {
                        None
                    };
                    Ok(RecordDocs { record, fields })
                }
                // Otherwise it is invalid
                _ => Err(E::custom("invalid docs".to_string())),
            }
        }
    }

    impl<E> From<SuspendedType> for Result<Type, E>
    where
        E: de::Error,
    {
        fn from(suspended_type: SuspendedType) -> Self {
            match suspended_type {
                // Basic and symbolic kinds
                SuspendedType::Str(s) => match s.as_str() {
                    // Basic kinds
                    "()" => Ok(Type::Unit),
                    "bool" => Ok(Type::Bool),
                    "u8" => Ok(Type::U8),
                    "u16" => Ok(Type::U16),
                    "u32" => Ok(Type::U32),
                    "u64" => Ok(Type::U64),
                    "u128" => Ok(Type::U128),
                    "i8" => Ok(Type::I8),
                    "i16" => Ok(Type::I16),
                    "i32" => Ok(Type::I32),
                    "i64" => Ok(Type::I64),
                    "i128" => Ok(Type::I128),
                    "f32" => Ok(Type::F32),
                    "f64" => Ok(Type::F64),
                    "string" => Ok(Type::String),

                    // Symbolic kinds
                    sym if sym.starts_with('$') => Ok(Type::from(Symbolic::new(&sym[1..]))),

                    // Generic kinds
                    gen if gen.starts_with('^') => Ok(Type::from(Generic::new(&gen[1..]))),

                    // Invalid kinds
                    k => Err(E::custom(format!("unexpected kind `{k}`"))),
                },

                // Maps
                SuspendedType::Map(map) => {
                    // Kind spec
                    let k = map.get("k").ok_or(de::Error::custom("expected `k`"))?;
                    // Type spec
                    let t = map.get("t").ok_or(de::Error::custom("expected `t`"))?;
                    // Optional name
                    let n = match map.get("n") {
                        Some(SuspendedType::Str(n)) => n.clone(),
                        Some(_) => return Err(de::Error::custom("invalid name")),
                        None => "_".to_string(),
                    };
                    // Optional doc
                    let doc = map.get("doc").cloned();

                    // Check the kind
                    match k {
                        SuspendedType::Str(k) => match k.as_str() {
                            // Product types
                            "array" => {
                                let inner_type =
                                    <SuspendedType as Into<Result<Type, E>>>::into(t.clone())?;
                                Ok(Type::from(Array::new(inner_type)))
                            }
                            "λ" | "func" => {
                                let ab = match t {
                                    SuspendedType::Seq(seq) if seq.len() == 2 => [
                                        <SuspendedType as Into<Result<Type, E>>>::into(
                                            seq[0].clone(),
                                        ),
                                        <SuspendedType as Into<Result<Type, E>>>::into(
                                            seq[1].clone(),
                                        ),
                                    ],
                                    _ => return Err(de::Error::custom("expected func type")),
                                };
                                match ab {
                                    [Ok(a), Ok(b)] => Ok(Type::from(Func::new(a, b))),
                                    _ => Err(de::Error::custom("invalid func type")),
                                }
                            }
                            "record" => {
                                let fields = match t {
                                    SuspendedType::Seq(seq) => Fields::from(
                                        seq.iter()
                                            .map(|t| {
                                                <SuspendedType as Into<Result<Type, E>>>::into(
                                                    t.clone(),
                                                )
                                                .map(UnnamedField::new)
                                            })
                                            .collect::<Result<Vec<_>, _>>()?,
                                    ),
                                    SuspendedType::Map(map) => Fields::from(
                                        map.iter()
                                            .map(|(n, t)| {
                                                <SuspendedType as Into<Result<Type, E>>>::into(
                                                    t.clone(),
                                                )
                                                .map(|t| NamedField::new(n, t))
                                            })
                                            .collect::<Result<Vec<_>, _>>()?,
                                    ),
                                    _ => return Err(de::Error::custom("expected record type")),
                                };
                                let mut rec = Record::new(n, fields);
                                rec.doc = doc.map(|d| d.into()).transpose()?;
                                Ok(Type::from(rec))
                            }
                            "tuple" => {
                                let tuple = match t {
                                    SuspendedType::Seq(seq) => seq
                                        .iter()
                                        .map(|t| {
                                            <SuspendedType as Into<Result<Type, E>>>::into(
                                                t.clone(),
                                            )
                                            .map(UnnamedField::new)
                                        })
                                        .collect::<Result<Vec<_>, _>>()?,
                                    _ => return Err(de::Error::custom("expected tuple type")),
                                };
                                Ok(Type::from(Tuple::new(tuple)))
                            }

                            // Sum types
                            "enum" => {
                                let variants = match t {
                                    SuspendedType::Seq(seq) => seq
                                        .iter()
                                        .map(|t| match t {
                                            SuspendedType::Str(n) => Ok(EnumVariant::new(n)),
                                            SuspendedType::Map(map) => {
                                                if map.len() != 1 {
                                                    return Err(de::Error::custom(
                                                        "invalid enum variant",
                                                    ));
                                                }
                                                match map.iter().next().unwrap() {
                                                    (n, SuspendedType::U64(v)) => {
                                                        Ok(EnumVariant::with_const_value(
                                                            n.clone(),
                                                            *v as u8,
                                                        ))
                                                    }
                                                    _ => Err(de::Error::custom(
                                                        "invalid enum variant",
                                                    )),
                                                }
                                            }
                                            _ => Err(de::Error::custom("expected enum variant")),
                                        })
                                        .collect::<Result<Vec<_>, _>>()?,
                                    _ => return Err(de::Error::custom("expected enum type")),
                                };
                                let enm = Enum::new(n, variants);
                                // TODO: Support documentation
                                // enm.doc = doc;
                                Ok(Type::from(enm))
                            }
                            "fallible" => {
                                let (t, e) = match t {
                                    SuspendedType::Seq(seq) if seq.len() == 2 => (
                                        <SuspendedType as Into<Result<Type, E>>>::into(
                                            seq[0].clone(),
                                        )?,
                                        <SuspendedType as Into<Result<Type, E>>>::into(
                                            seq[1].clone(),
                                        )?,
                                    ),
                                    _ => return Err(de::Error::custom("expected fallible type")),
                                };
                                Ok(Type::from(Fallible::new(t, e)))
                            }
                            "optional" => {
                                let inner_type =
                                    <SuspendedType as Into<Result<Type, E>>>::into(t.clone())?;
                                Ok(Type::from(Optional::new(inner_type)))
                            }
                            "union" => {
                                let variants = match t {
                                    SuspendedType::Seq(seq) => seq
                                        .iter()
                                        .map(|t| match t {
                                            SuspendedType::Str(n) => Ok(UnionVariant::new(n)),
                                            SuspendedType::Map(map) => {
                                                if map.len() != 1 {
                                                    return Err(de::Error::custom(
                                                        "invalid union variant",
                                                    ));
                                                }
                                                match map.iter().next().unwrap() {
                                                    (n, SuspendedType::Seq(seq)) => {
                                                        Ok(UnionVariant::with_fields(
                                                            n.clone(),
                                                            seq.iter()
                                                                .map(|t| {
                                                                    <SuspendedType as Into<
                                                                        Result<Type, E>,
                                                                    >>::into(
                                                                        t.clone()
                                                                    )
                                                                    .map(UnnamedField::new)
                                                                })
                                                                .collect::<Result<Vec<_>, _>>()?,
                                                        ))
                                                    }
                                                    (n, SuspendedType::Map(map)) => {
                                                        Ok(UnionVariant::with_fields(
                                                            n.clone(),
                                                            map.iter()
                                                                .map(|(n, t)| {
                                                                    <SuspendedType as Into<
                                                                        Result<Type, E>,
                                                                    >>::into(
                                                                        t.clone()
                                                                    )
                                                                    .map(|t| NamedField::new(n, t))
                                                                })
                                                                .collect::<Result<Vec<_>, _>>()?,
                                                        ))
                                                    }
                                                    _ => Err(de::Error::custom(
                                                        "invalid union variant",
                                                    )),
                                                }
                                            }
                                            _ => Err(de::Error::custom("expected union variant")),
                                        })
                                        .collect::<Result<Vec<_>, _>>()?,
                                    _ => return Err(de::Error::custom("expected union type")),
                                };
                                let union = Union::new(n, variants);
                                // TODO: Support documentation
                                // union.doc = doc;
                                Ok(Type::from(union))
                            }
                            "@" | "ptr" => {
                                let inner_type =
                                    <SuspendedType as Into<Result<Type, E>>>::into(t.clone())?;
                                Ok(Type::from(Ptr::new(inner_type)))
                            }
                            "alias" => {
                                let inner_type =
                                    <SuspendedType as Into<Result<Type, E>>>::into(t.clone())?;
                                Ok(Type::from(Alias::new(n, inner_type)))
                            }
                            _ => Err(de::Error::custom(format!("unexpected kind `{k}`"))),
                        },
                        _ => Err(de::Error::custom("invalid kind".to_string())),
                    }
                }

                // No kind is specified
                _ => Err(E::custom("invalid kind".to_string())),
            }
        }
    }

    impl<'de> Deserialize<'de> for SuspendedType {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(SuspendedTypeVisitor)
        }
    }

    impl<'de> Deserialize<'de> for Type {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(SuspendedTypeVisitor)?.into()
        }
    }

    impl<'de> Deserialize<'de> for Fields {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(FieldsVisitor)
        }
    }

    impl<'de> Deserialize<'de> for EnumVariant {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(EnumVariantVisitor)
        }
    }

    impl<'de> Deserialize<'de> for UnionVariant {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(UnionVariantVisitor)
        }
    }

    struct SuspendedTypeVisitor;

    impl<'de> Visitor<'de> for SuspendedTypeVisitor {
        type Value = SuspendedType;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("valid type")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SuspendedType::Null)
        }

        fn visit_u8<E>(self, value: u8) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SuspendedType::U64(value as u64))
        }

        fn visit_u16<E>(self, value: u16) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SuspendedType::U64(value as u64))
        }

        fn visit_u32<E>(self, value: u32) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SuspendedType::U64(value as u64))
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SuspendedType::U64(value))
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(SuspendedType::Str(value.to_string()))
        }

        fn visit_seq<S>(self, mut access: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut seq = Vec::new();
            while let Some(elem) = access.next_element::<SuspendedType>()? {
                seq.push(elem);
            }
            Ok(SuspendedType::Seq(seq))
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut map = HashMap::new();
            loop {
                let key = access.next_key::<String>()?;
                match key {
                    Some(key) => {
                        map.insert(key, access.next_value::<SuspendedType>()?);
                    }
                    None => break,
                }
            }
            Ok(SuspendedType::Map(map))
        }
    }

    struct FieldsVisitor;

    impl<'de> Visitor<'de> for FieldsVisitor {
        type Value = Fields;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("valid fields")
        }

        fn visit_seq<S>(self, mut access: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut unnamed_fields = Vec::new();
            while let Some(t) = access.next_element::<Type>()? {
                unnamed_fields.push(UnnamedField::new(t));
            }
            Ok(Fields::unnamed(unnamed_fields))
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut named_fields = Vec::new();
            while let Some((n, t)) = access.next_entry::<String, Type>()? {
                named_fields.push(NamedField::new(n, t));
            }
            Ok(Fields::named(named_fields))
        }
    }

    struct EnumVariantVisitor;

    impl<'de> Visitor<'de> for EnumVariantVisitor {
        type Value = EnumVariant;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("valid enum variant")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(EnumVariant::new(value))
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            use serde::de::Error;

            match access.next_key::<String>()? {
                Some(n) => access
                    .next_value::<u8>()
                    .map(|v| EnumVariant::with_const_value(n, v)),
                _ => Err(M::Error::custom("expected const value in enum variant")),
            }
        }
    }

    struct UnionVariantVisitor;

    impl<'de> Visitor<'de> for UnionVariantVisitor {
        type Value = UnionVariant;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("valid union variant")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(UnionVariant::new(value))
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            use serde::de::Error;

            match access.next_key::<String>()? {
                Some(n) => access
                    .next_value::<Fields>()
                    .map(|fields| UnionVariant::with_fields(n, fields)),
                _ => Err(M::Error::custom("expected fields in union variant")),
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{
        product::Record,
        sum::{Enum, EnumVariant, Union, UnionVariant},
        type_info::{Type, TypeInfo},
        Fields, Lambda, Ptr,
    };

    #[test]
    fn test_unit() {
        let t = <()>::t();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#""()""#);
    }

    #[test]
    fn test_array() {
        let t = Vec::<u8>::t();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"k":"array","t":"u8"}"#);
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(r#"{"t":"u8","k":"array"}"#).unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_func() {
        let t = Lambda::<(u64, u64), u64>::t();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"k":"λ","t":[{"k":"tuple","t":["u64","u64"]},"u64"]}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"t":[{"k":"tuple","t":["u64","u64"]},"u64"],"k":"λ"}"#,
        )
        .unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"t":[{"k":"tuple","t":["u64","u64"]},"u64"],"k":"func"}"#,
        )
        .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_record() {
        let t = Type::Record(Record::new("Foo", [u8::t(), Vec::<u8>::t()]));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"k":"record","t":["u8",{"k":"array","t":"u8"}],"n":"Foo"}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"n":"Foo","t":["u8",{"t":"u8","k":"array"}],"k":"record"}"#,
        )
        .unwrap();
        assert_eq!(t, u);

        let t = Type::Record(Record::new(
            "Foo",
            [("bar", u8::t()), ("baz", Vec::<u8>::t())],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"k":"record","t":{"bar":"u8","baz":{"k":"array","t":"u8"}},"n":"Foo"}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"n":"Foo","t":{"baz":{"t":"u8","k":"array"},"bar":"u8"},"k":"record"}"#,
        )
        .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_record_with_docs() {
        let t = Type::Record(Record::with_doc(
            "Docs for foo",
            "Foo",
            [u8::t(), Vec::<u8>::t()],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"doc":{"record":"Docs for foo"},"k":"record","t":["u8",{"k":"array","t":"u8"}],"n":"Foo"}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"n":"Foo","t":["u8",{"t":"u8","k":"array"}],"doc":{"record":"Docs for foo"},"k":"record"}"#,
        )
        .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_record_with_field_docs() {
        let t = Type::Record(Record::with_doc(
            [("x", "This is field x"), ("y", "This is field y")],
            "Foo",
            [("x", u8::t()), ("y", Vec::<u8>::t())],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert!(
            (json
                == r#"{"doc":{"fields":{"x":"This is field x","y":"This is field y"}},"k":"record","t":{"x":"u8","y":{"k":"array","t":"u8"}},"n":"Foo"}"#)
                || (json
                    == r#"{"doc":{"fields":{"y":"This is field y","x":"This is field x"}},"k":"record","t":{"x":"u8","y":{"k":"array","t":"u8"}},"n":"Foo"}"#)
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"t":{"x":"u8","y":{"t":"u8","k":"array"}},"k":"record","n":"Foo","doc":{"fields":{"x":"This is field x","y":"This is field y"}}}"#
        )
        .unwrap();
        assert_eq!(t, u);

        let t = Type::Record(Record::with_doc(
            ["This is field 1", "This is field 2"],
            "Foo",
            [u8::t(), Vec::<u8>::t()],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"doc":{"fields":["This is field 1","This is field 2"]},"k":"record","t":["u8",{"k":"array","t":"u8"}],"n":"Foo"}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"k":"record","t":["u8",{"k":"array","t":"u8"}],"n":"Foo","doc":{"fields":["This is field 1","This is field 2"]}}"#
        )
        .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_tuple() {
        let t = <(u8, Vec<u8>)>::t();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"k":"tuple","t":["u8",{"k":"array","t":"u8"}]}"#);
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(r#"{"t":["u8",{"t":"u8","k":"array"}],"k":"tuple"}"#)
            .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_enum() {
        let t = Type::Enum(Enum::new(
            "Foo",
            [EnumVariant::new("Bar"), EnumVariant::new("Baz")],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"k":"enum","t":["Bar","Baz"],"n":"Foo"}"#);
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u =
            serde_json::from_str::<Type>(r#"{"n":"Foo","k":"enum","t":["Bar","Baz"]}"#).unwrap();
        assert_eq!(t, u);

        let t = Type::Enum(Enum::new(
            "Foo",
            [
                EnumVariant::with_const_value("Bar", 0),
                EnumVariant::with_const_value("Baz", 1),
            ],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"k":"enum","t":[{"Bar":0},{"Baz":1}],"n":"Foo"}"#);
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(r#"{"n":"Foo","k":"enum","t":[{"Bar":0},{"Baz":1}]}"#)
            .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_fallible() {
        let t = Result::<(u8, Vec<u8>), Option<String>>::t();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"k":"fallible","t":[{"k":"tuple","t":["u8",{"k":"array","t":"u8"}]},{"k":"optional","t":"string"}]}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"t":[{"t":["u8",{"t":"u8","k":"array"}],"k":"tuple"},{"t":"string","k":"optional"}],"k":"fallible"}"#
        )
        .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_optional() {
        let t = Option::<(u8, Vec<u8>)>::t();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"k":"optional","t":{"k":"tuple","t":["u8",{"k":"array","t":"u8"}]}}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"t":{"t":["u8",{"t":"u8","k":"array"}],"k":"tuple"},"k":"optional"}"#,
        )
        .unwrap();
        assert_eq!(t, u);
    }

    #[test]
    fn test_union() {
        let t = Type::Union(Union::new(
            "Foo",
            [
                UnionVariant::with_fields("X", [Type::U8, Vec::<u8>::t()]),
                UnionVariant::with_fields("Y", [("bar", Type::U8), ("baz", Type::U8)]),
                UnionVariant::new("Z"),
            ],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"k":"union","t":[{"X":["u8",{"k":"array","t":"u8"}]},{"Y":{"bar":"u8","baz":"u8"}},"Z"],"n":"Foo"}"#
        );
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(
            r#"{"n":"Foo","k":"union","t":["Z",{"Y":{"bar":"u8","baz":"u8"}},{"X":["u8",{"t":"u8","k":"array"}]}]}"#,
        )
        .unwrap();
        assert_eq!(t, u);

        let v = serde_json::from_str::<serde_json::Value>(r#"{"X":[1,[2,3]]}"#).unwrap();
        assert!(t.is_compat(Some(&v)).is_ok());

        let v = serde_json::from_str::<serde_json::Value>(r#"{"Y":{"bar":1,"baz":2}}"#).unwrap();
        assert!(t.is_compat(Some(&v)).is_ok());

        let v = serde_json::from_str::<serde_json::Value>(r#""Z""#).unwrap();
        assert!(t.is_compat(Some(&v)).is_ok());

        let t = Type::Union(Union::new(
            "Foo",
            [
                UnionVariant::with_fields(
                    "W",
                    [Type::Record(Record::new(
                        "Object",
                        Fields::named(vec![
                            ("path", Type::String),
                            ("format", Type::String),
                            ("size", Type::U32),
                        ]),
                    ))],
                ),
                UnionVariant::with_fields("X", [Vec::<u8>::t()]),
                UnionVariant::with_fields("Y", [("bar", Type::U8), ("baz", Type::U8)]),
                UnionVariant::new("Z"),
            ],
        ));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"k":"union","t":[{"W":[{"k":"record","t":{"path":"string","format":"string","size":"u32"},"n":"Object"}]},{"X":[{"k":"array","t":"u8"}]},{"Y":{"bar":"u8","baz":"u8"}},"Z"],"n":"Foo"}"#
        );

        let v = serde_json::from_str::<serde_json::Value>(
            r#"{"W":{"path":"~/hello","format":"bin","size":1234}}"#,
        )
        .unwrap();
        assert!(t.is_compat(Some(&v)).is_ok());

        let v = serde_json::from_str::<serde_json::Value>(r#"{"X":[1,2,3,4,5]}"#).unwrap();
        assert!(t.is_compat(Some(&v)).is_ok());

        let v = serde_json::from_str::<serde_json::Value>(r#"{"Y":{"bar":1,"baz":2}}"#).unwrap();
        assert!(t.is_compat(Some(&v)).is_ok());

        let v = serde_json::from_str::<serde_json::Value>(r#""Z""#).unwrap();
        assert!(t.is_compat(Some(&v)).is_ok());
    }

    #[test]
    fn test_ptr() {
        let t = Type::from(Ptr::new(u8::t()));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"k":"@","t":"u8"}"#);
        let u = serde_json::from_str::<Type>(&json).unwrap();
        assert_eq!(t, u);
        let u = serde_json::from_str::<Type>(r#"{"t":"u8","k":"@"}"#).unwrap();
        assert_eq!(t, u);
    }
}
