use serde_json::{Map, Value};

use crate::{
    Alias, Array, Enum, EnumVariant, Fallible, Fields, Func, Generic, Lambda, Optional, Ptr,
    Record, Symbolic, Tuple, Type, Union, UnionVariant,
};

pub trait ValueVisitor {
    type Error;

    fn visit_unit(&mut self, _val: &Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_bool(&mut self, _val: bool) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u8(&mut self, _val: u8) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u16(&mut self, _val: u16) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u32(&mut self, _val: u32) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u64(&mut self, _val: u64) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u128(&mut self, _val: u128) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i8(&mut self, _val: i8) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i16(&mut self, _val: i16) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i32(&mut self, _val: i32) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i64(&mut self, _val: i64) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i128(&mut self, _val: i128) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_f32(&mut self, _val: f32) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_f64(&mut self, _val: f64) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_string(&mut self, _val: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_array(&mut self, _arr: &Array, _val: &[Value]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_func(&mut self, _func: &Func, _val: &Lambda<Value, Value>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_record_with_named_fields(
        &mut self,
        _rec: &Record,
        _val: &Map<String, Value>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_record_with_unnamed_fields(
        &mut self,
        _rec: &Record,
        _val: &[Value],
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_tuple(&mut self, _tup: &Tuple, _val: &[Value]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_enum_variant_string(
        &mut self,
        _enm: &Enum,
        _var: &EnumVariant,
        _val: &str,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_enum_variant_const_value(
        &mut self,
        _enm: &Enum,
        _var: &EnumVariant,
        _val: u8,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_fallible_ok(&mut self, _fall: &Fallible, _val: &Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_fallible_err(&mut self, _fall: &Fallible, _val: &Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_optional(&mut self, _opt: &Optional, _val: Option<&Value>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_union_variant_string(
        &mut self,
        _union: &Union,
        _var: &UnionVariant,
        _val: &str,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_union_variant_fields(
        &mut self,
        _union: &Union,
        _var: &UnionVariant,
        _val: &Map<String, Value>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_ptr(&mut self, _ptr: &Ptr, _val: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_symbolic(&mut self, _sym: &Symbolic, _val: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_generic(&mut self, _gen: &Generic, _val: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_alias(&mut self, _alias: &Alias, _val: &Value) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub fn walk_value<V>(v: &mut V, t: &Type, val: &Value) -> Result<(), V::Error>
where
    V: ValueVisitor,
{
    if let Err(err) = t.is_compat(Some(val)) {
        println!("err: {:?}", err);
        println!("t: {:?}", t);
        println!("val: {:?}", val);
        todo!()
    }

    match t {
        Type::Unit => v.visit_unit(val),
        Type::Bool => v.visit_bool(val.as_bool().expect("value should be bool")),
        Type::U8 => v.visit_u8(val.as_u64().expect("value should be u8") as u8),
        Type::U16 => v.visit_u16(val.as_u64().expect("value should be u16") as u16),
        Type::U32 => v.visit_u32(val.as_u64().expect("value should be u32") as u32),
        Type::U64 => v.visit_u64(val.as_u64().expect("value should be u64")),
        Type::U128 => v.visit_u128(val.as_u64().expect("value should be u128") as u128),
        Type::I8 => v.visit_i8(val.as_i64().expect("value should be i8") as i8),
        Type::I16 => v.visit_i16(val.as_i64().expect("value should be i16") as i16),
        Type::I32 => v.visit_i32(val.as_i64().expect("value should be i32") as i32),
        Type::I64 => v.visit_i64(val.as_i64().expect("value should be i64")),
        Type::I128 => v.visit_i128(val.as_i64().expect("value should be i128") as i128),
        Type::F32 => v.visit_f32(val.as_f64().expect("value should be f32") as f32),
        Type::F64 => v.visit_f64(val.as_f64().expect("value should be f64")),
        Type::String => v.visit_string(val.as_str().expect("value should be string")),
        Type::Array(arr) => {
            let val = val.as_array().expect("value should be array");
            v.visit_array(arr, val)?;
            for val in val {
                walk_value(v, &arr.t, val)?;
            }
            Ok(())
        }
        Type::Func(func) => {
            let val = serde_json::from_value::<Lambda<Value, Value>>(val.clone())
                .expect("value should be function");
            v.visit_func(func, &val)
        }
        Type::Record(rec) => match &rec.fields {
            Fields::Named(fields) => {
                let val = val.as_object().expect("value should be record");
                v.visit_record_with_named_fields(rec, val)?;
                for field in fields.iter() {
                    walk_value(v, &field.t, val.get(&field.n).unwrap_or(&Value::Null))?;
                }
                Ok(())
            }
            Fields::Unnamed(fields) => {
                let val = val.as_array().expect("value should be record");
                v.visit_record_with_unnamed_fields(rec, val)?;
                for (i, field) in fields.iter().enumerate() {
                    walk_value(
                        v,
                        &field.t,
                        val.get(i)
                            .unwrap_or_else(|| panic!("value should have field at index: {}", i)),
                    )?;
                }
                Ok(())
            }
        },
        Type::Tuple(tup) => {
            let val = val.as_array().expect("value should be tuple");
            v.visit_tuple(tup, val)?;
            for (i, field) in tup.fields.iter().enumerate() {
                walk_value(
                    v,
                    &field.t,
                    val.get(i).expect("value should have tuple field"),
                )?;
            }
            Ok(())
        }
        Type::Enum(enm) => match val {
            Value::String(string) => {
                for variant in &enm.variants {
                    if string == &variant.n {
                        return v.visit_enum_variant_string(enm, variant, string);
                    }
                }
                panic!("value should be enum variant (string)")
            }
            Value::Number(num) => {
                for variant in &enm.variants {
                    if num.as_u64() == variant.v.map(|y| y as u64) {
                        return v.visit_enum_variant_const_value(
                            enm,
                            variant,
                            num.as_u64().unwrap() as u8,
                        );
                    }
                }
                panic!("value should be enum variant (const value)")
            }
            _ => panic!("value should be enum variant"),
        },
        Type::Fallible(fall) => match val {
            Value::Object(object) => {
                if let Some(ok) = object.get("Ok") {
                    v.visit_fallible_ok(fall, ok)?;
                    walk_value(v, &fall.ok, ok)
                } else if let Some(err) = object.get("Err") {
                    v.visit_fallible_err(fall, err)?;
                    walk_value(v, &fall.err, err)
                } else {
                    panic!("value should be fallible")
                }
            }
            _ => panic!("value should be fallible"),
        },
        Type::Optional(opt) => {
            if val.is_null() {
                v.visit_optional(opt, None)
            } else {
                v.visit_optional(opt, Some(val))?;
                walk_value(v, &opt.t, val)
            }
        }
        Type::Union(union) => match val {
            Value::String(string) => {
                for variant in &union.variants {
                    if variant.fields.is_none() && string == &variant.n {
                        return v.visit_union_variant_string(union, variant, string);
                    }
                }
                panic!("value should be union variant (string)")
            }
            Value::Object(object) => {
                for variant in &union.variants {
                    if object
                        .get(&variant.n)
                        .and_then(|object_fields| {
                            variant
                                .fields
                                .as_ref()
                                .map(|variant_fields| match variant_fields {
                                    Fields::Unnamed(variant_unnamed_fields)
                                        if variant_unnamed_fields.len() == 1 =>
                                    {
                                        variant_unnamed_fields.fields[0]
                                            .t
                                            .is_compat(Some(object_fields))
                                            .is_ok()
                                    }
                                    _ => variant_fields.is_compat(Some(object_fields)).is_ok(),
                                })
                        })
                        .unwrap_or(false)
                    {
                        // TODO: make union and variant optional within visit_union_variant_string
                        v.visit_union_variant_fields(union, variant, object)?;

                        if let Some(fields) = &variant.fields {
                            match fields {
                                Fields::Named(fields) => {
                                    for field in fields.iter() {
                                        walk_value(
                                            v,
                                            &field.t,
                                            object
                                                .get(&variant.n)
                                                .and_then(|object_fields| {
                                                    object_fields.get(&field.n)
                                                })
                                                .unwrap_or(&mut Value::Null),
                                        )?;
                                    }
                                }
                                Fields::Unnamed(fields) if fields.len() == 1 => {
                                    walk_value(
                                        v,
                                        &fields.fields[0].t,
                                        object.get(&variant.n).unwrap(),
                                    )?;
                                }
                                Fields::Unnamed(fields) => {
                                    for (i, field) in fields.iter().enumerate() {
                                        walk_value(
                                            v,
                                            &field.t,
                                            object
                                                .get(&variant.n)
                                                .and_then(|object_fields| object_fields.get(i))
                                                .unwrap_or(&mut Value::Null),
                                        )?;
                                    }
                                }
                            };
                        }

                        return Ok(());
                    }
                }
                panic!("value should be union variant (fields)")
            }
            _ => panic!("value should be union variant"),
        },
        Type::Ptr(p) => v.visit_ptr(p, val.as_str().expect("value should be pointer")),
        Type::Symbolic(sym) => v.visit_symbolic(sym, val.as_str().expect("value should be symbol")),
        Type::Generic(gen) => v.visit_generic(gen, val.as_str().expect("value should be generic")),
        Type::Alias(alias) => {
            v.visit_alias(alias, val)?;
            walk_value(v, &alias.t, val)
        }
    }
}

pub trait MutableValueVisitor {
    type Error;

    fn visit_unit(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }
    fn visit_bool(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u8(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u16(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u32(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u64(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u128(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i8(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i16(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i32(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i64(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i128(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_f32(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_f64(&mut self, _val: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_string(&mut self, _val: &mut str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_array(&mut self, _arr: &mut Array, _val: &mut [Value]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_func(
        &mut self,
        _func: &Func,
        _val: &mut Lambda<Value, Value>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_record_with_named_fields(
        &mut self,
        _rec: &mut Record,
        _val: &mut Map<String, Value>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_record_with_unnamed_fields(
        &mut self,
        _rec: &mut Record,
        _val: &mut [Value],
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_tuple(&mut self, _tup: &mut Tuple, _val: &mut [Value]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_enum_variant_string(
        &mut self,
        _enm: &Enum,
        _var: &EnumVariant,
        _val: &str,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_enum_variant_const_value(
        &mut self,
        _enm: &Enum,
        _var: &EnumVariant,
        _val: u8,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_fallible_ok(&mut self, _fall: &Fallible, _ok: &mut Value) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_fallible_err(
        &mut self,
        _fall: &Fallible,
        _err: &mut Value,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_optional(
        &mut self,
        _opt: &Optional,
        _val: &mut Option<&mut Value>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_union_variant_string(
        &mut self,
        _union: &Union,
        _var: &UnionVariant,
        _val: &mut String,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_union_variant_fields(
        &mut self,
        _var: &mut UnionVariant,
        _val: &mut Map<String, Value>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_ptr(&mut self, _ptr: &Ptr, _val: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_symbolic(&mut self, _sym: &Symbolic, _val: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_generic(&mut self, _gen: &Generic, _val: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_alias(&mut self, _alias: &Alias, _val: &Value) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub fn walk_value_mut<V>(v: &mut V, t: &mut Type, val: &mut Value) -> Result<(), V::Error>
where
    V: MutableValueVisitor,
{
    if let Err(err) = t.is_compat(Some(val)) {
        println!("err: {:?}", err);
        println!("t: {:?}", t);
        println!("val: {:?}", val);
        todo!()
    }

    match t {
        Type::Unit => v.visit_unit(val),
        Type::Bool => v.visit_bool(val),
        Type::U8 => v.visit_u8(val),
        Type::U16 => v.visit_u16(val),
        Type::U32 => v.visit_u32(val),
        Type::U64 => v.visit_u64(val),
        Type::U128 => v.visit_u128(val),
        Type::I8 => v.visit_i8(val),
        Type::I16 => v.visit_i16(val),
        Type::I32 => v.visit_i32(val),
        Type::I64 => v.visit_i64(val),
        Type::I128 => v.visit_i128(val),
        Type::F32 => v.visit_f32(val),
        Type::F64 => v.visit_f64(val),
        Type::String => v.visit_string(match val {
            Value::String(string) => string,
            _ => panic!("value should be string"),
        }),
        Type::Array(ref mut arr) => {
            let val = val.as_array_mut().expect("value should be array");
            v.visit_array(arr, val)?;
            for val in val {
                walk_value_mut(v, &mut arr.t, val)?;
            }
            Ok(())
        }
        Type::Func(func) => {
            let val = &mut serde_json::from_value::<Lambda<Value, Value>>(val.clone())
                .expect("value should be function");
            v.visit_func(func, val)
        }

        Type::Record(rec) => {
            if matches!(rec.fields, Fields::Named(_)) {
                let val = val.as_object_mut().expect("value should be record");
                v.visit_record_with_named_fields(rec, val)?;

                // always true; only to avoid a double borrow
                if let Fields::Named(named_fields) = &mut rec.fields {
                    for field in named_fields.iter_mut() {
                        walk_value_mut(
                            v,
                            &mut field.t,
                            val.get_mut(&field.n).unwrap_or(&mut Value::Null),
                        )?;
                    }
                }
                Ok(())
            } else if matches!(rec.fields, Fields::Unnamed(_)) {
                let val = val.as_array_mut().expect("value should be record");
                v.visit_record_with_unnamed_fields(rec, val)?;

                // always true; only to avoid a double borrow
                if let Fields::Unnamed(unnamed_fields) = &mut rec.fields {
                    for (i, field) in unnamed_fields.iter_mut().enumerate() {
                        walk_value_mut(
                            v,
                            &mut field.t,
                            val.get_mut(i).expect("value should have record field"),
                        )?;
                    }
                }
                Ok(())
            } else {
                Ok(())
            }
        }
        Type::Tuple(tup) => {
            let val = val.as_array_mut().expect("value should be tuple");
            v.visit_tuple(tup, val)?;
            for (i, field) in tup.fields.iter_mut().enumerate() {
                walk_value_mut(
                    v,
                    &mut field.t,
                    val.get_mut(i).expect("value should have tuple field"),
                )?;
            }
            Ok(())
        }
        Type::Enum(enm) => match val {
            Value::String(string) => {
                for variant in &enm.variants {
                    if string == &variant.n {
                        // TODO: make optional mutable within visit_enum_variant_string
                        return v.visit_enum_variant_string(enm, variant, string);
                    }
                }
                panic!("value should be enum variant (string)")
            }
            Value::Number(num) => {
                for variant in &enm.variants {
                    if num.as_u64() == variant.v.map(|y| y as u64) {
                        // TODO: make enum mutable within visit_enum_variant_const_value
                        return v.visit_enum_variant_const_value(
                            enm,
                            variant,
                            num.as_u64().unwrap() as u8,
                        );
                    }
                }
                panic!("value should be enum variant (const value)")
            }
            _ => panic!("value should be enum variant"),
        },
        Type::Fallible(fall) => match val {
            Value::Object(object) => {
                if let Some(ok) = object.get_mut("Ok") {
                    v.visit_fallible_ok(fall, ok)?;
                    walk_value_mut(v, fall.ok.as_mut(), ok)
                } else if let Some(err) = object.get_mut("Err") {
                    v.visit_fallible_err(fall, err)?;
                    walk_value_mut(v, fall.err.as_mut(), err)
                } else {
                    panic!("value should be fallible")
                }
            }
            _ => panic!("value should be fallible"),
        },
        Type::Optional(opt) => {
            if val.is_null() {
                // TODO: make enum mutable within visit_optional
                v.visit_optional(opt, &mut None)
            } else {
                v.visit_optional(opt, &mut Some(val))?;
                walk_value_mut(v, &mut opt.t, val)
            }
        }
        Type::Union(union) => match val {
            Value::String(string) => {
                for variant in &union.variants {
                    if variant.fields.is_none() && string == &variant.n {
                        // TODO: make union and variant optional within visit_union_variant_string
                        return v.visit_union_variant_string(union, variant, string);
                    }
                }
                panic!("value should be union variant (string)")
            }
            Value::Object(object) => {
                for variant in &mut union.variants {
                    if object
                        .get(&variant.n)
                        .and_then(|object_fields| {
                            variant
                                .fields
                                .as_ref()
                                .map(|variant_fields| match variant_fields {
                                    Fields::Unnamed(variant_unnamed_fields)
                                        if variant_unnamed_fields.len() == 1 =>
                                    {
                                        variant_unnamed_fields.fields[0]
                                            .t
                                            .is_compat(Some(object_fields))
                                            .is_ok()
                                    }
                                    _ => variant_fields.is_compat(Some(object_fields)).is_ok(),
                                })
                        })
                        .unwrap_or(false)
                    {
                        // TODO: make union and variant optional within visit_union_variant_string
                        v.visit_union_variant_fields(variant, object)?;

                        if let Some(fields) = &mut variant.fields {
                            match fields {
                                Fields::Named(fields) => {
                                    for field in fields.iter_mut() {
                                        walk_value_mut(
                                            v,
                                            &mut field.t,
                                            object
                                                .get_mut(&variant.n)
                                                .and_then(|object_fields| {
                                                    object_fields.get_mut(&field.n)
                                                })
                                                .unwrap_or(&mut Value::Null),
                                        )?;
                                    }
                                }
                                Fields::Unnamed(fields) if fields.len() == 1 => {
                                    walk_value_mut(
                                        v,
                                        &mut fields.fields[0].t,
                                        object.get_mut(&variant.n).unwrap(),
                                    )?;
                                }
                                Fields::Unnamed(fields) => {
                                    for (i, field) in fields.iter_mut().enumerate() {
                                        walk_value_mut(
                                            v,
                                            &mut field.t,
                                            object
                                                .get_mut(&variant.n)
                                                .and_then(|object_fields| object_fields.get_mut(i))
                                                .unwrap_or(&mut Value::Null),
                                        )?;
                                    }
                                }
                            };
                        }

                        return Ok(());
                    }
                }
                panic!("value should be union variant (fields)")
            }
            _ => panic!("value should be union variant"),
        },
        // TODO: make the below functions take mutable parameters
        Type::Ptr(p) => v.visit_ptr(p, val.as_str().expect("value should be pointer")),
        Type::Symbolic(sym) => v.visit_symbolic(sym, val.as_str().expect("value should be symbol")),
        Type::Generic(gen) => v.visit_generic(gen, val.as_str().expect("value should be generic")),
        Type::Alias(alias) => {
            v.visit_alias(alias, val)?;
            walk_value_mut(v, &mut alias.t, val)
        }
    }
}

pub trait TypeVisitor {
    type Error;

    fn visit_unit(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_bool(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u8(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u16(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u32(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u64(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_u128(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i8(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i16(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i32(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i64(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_i128(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_f32(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_f64(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_string(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_array(&mut self, _arr: &Array) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_func(&mut self, _func: &Func) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_record_with_named_fields(&mut self, _rec: &Record) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_record_with_unnamed_fields(&mut self, _rec: &Record) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_tuple(&mut self, _tup: &Tuple) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_enum_variant_string(
        &mut self,
        _enm: &Enum,
        _var: &EnumVariant,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_enum_variant_const_value(
        &mut self,
        _enm: &Enum,
        _var: &EnumVariant,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_fallible(&mut self, _fall: &Fallible) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_optional(&mut self, _opt: &Optional) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_union_variant_string(&mut self, _var: &UnionVariant) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_union_variant_fields(&mut self, _var: &UnionVariant) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_ptr(&mut self, _p: &Ptr) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_symbolic(&mut self, _sym: &Symbolic) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_generic(&mut self, _gen: &Generic) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_alias(&mut self, _alias: &Alias) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub fn walk_type<V>(v: &mut V, t: &Type) -> Result<(), V::Error>
where
    V: TypeVisitor,
{
    match t {
        Type::Unit => v.visit_unit(),
        Type::Bool => v.visit_bool(),
        Type::U8 => v.visit_u8(),
        Type::U16 => v.visit_u16(),
        Type::U32 => v.visit_u32(),
        Type::U64 => v.visit_u64(),
        Type::U128 => v.visit_u128(),
        Type::I8 => v.visit_i8(),
        Type::I16 => v.visit_i16(),
        Type::I32 => v.visit_i32(),
        Type::I64 => v.visit_i64(),
        Type::I128 => v.visit_i128(),
        Type::F32 => v.visit_f32(),
        Type::F64 => v.visit_f64(),
        Type::String => v.visit_string(),
        Type::Array(arr) => {
            v.visit_array(arr)?;
            walk_type(v, &arr.t)
        }
        Type::Func(func) => {
            v.visit_func(func)?;
            walk_type(v, &func.a)?;
            walk_type(v, &func.b)
        }
        Type::Record(rec) => match &rec.fields {
            Fields::Named(fields) => {
                v.visit_record_with_named_fields(rec)?;
                for field in fields.iter() {
                    walk_type(v, &field.t)?;
                }
                Ok(())
            }
            Fields::Unnamed(fields) => {
                v.visit_record_with_unnamed_fields(rec)?;
                for field in fields.iter() {
                    walk_type(v, &field.t)?;
                }
                Ok(())
            }
        },
        Type::Tuple(tup) => {
            v.visit_tuple(tup)?;
            for field in tup.fields.iter() {
                walk_type(v, &field.t)?;
            }
            Ok(())
        }
        Type::Enum(enm) => {
            for variant in &enm.variants {
                match variant.v {
                    Some(_) => v.visit_enum_variant_const_value(enm, variant)?,
                    None => v.visit_enum_variant_string(enm, variant)?,
                };
            }
            Ok(())
        }
        Type::Fallible(fall) => {
            v.visit_fallible(fall)?;
            walk_type(v, &fall.ok)?;
            walk_type(v, &fall.err)?;
            Ok(())
        }
        Type::Optional(opt) => {
            v.visit_optional(opt)?;
            walk_type(v, &opt.t)
        }
        Type::Union(union) => {
            for variant in &union.variants {
                match &variant.fields {
                    Some(fields) => {
                        v.visit_union_variant_fields(variant)?;
                        match fields {
                            Fields::Named(fields) => {
                                for field in fields.iter() {
                                    walk_type(v, &field.t)?;
                                }
                            }
                            Fields::Unnamed(fields) => {
                                for field in fields.iter() {
                                    walk_type(v, &field.t)?;
                                }
                            }
                        }
                    }
                    None => v.visit_union_variant_string(variant)?,
                };
            }
            Ok(())
        }
        Type::Ptr(p) => {
            v.visit_ptr(p)?;
            walk_type(v, &p.t)
        }
        Type::Symbolic(sym) => v.visit_symbolic(sym),
        Type::Generic(gen) => v.visit_generic(gen),
        Type::Alias(alias) => {
            v.visit_alias(alias)?;
            walk_type(v, &alias.t)
        }
    }
}

#[macro_export]
macro_rules! unsigned_int_range_check {
    ($v: ident as $uint: ident else $err: ident) => {
        $v.and_then(|$v| $v.as_u64())
            .and_then(|$v| ($v <= $uint::MAX as u64).then_some(()))
            .ok_or(Error::$err {
                got: $v.cloned().unwrap_or(::serde_json::Value::Null),
            })
    };
}

#[macro_export]
macro_rules! signed_int_range_check {
    ($v: ident as $sint: ident else $err: ident) => {
        $v.and_then(|$v| $v.as_i64())
            .and_then(|$v| ($v >= $sint::MIN as i64 && $v <= $sint::MAX as i64).then_some(()))
            .ok_or(Error::$err {
                got: $v.cloned().unwrap_or(::serde_json::Value::Null),
            })
    };
}

#[macro_export]
macro_rules! float_range_check {
    ($v: ident as $f: ident else $err: ident) => {
        if let Some(x) = $v.and_then(|$v| $v.as_f64()) {
            if x >= $f::MIN as f64 && x <= $f::MAX as f64 {
                Ok(())
            } else {
                Err(Error::$err {
                    got: $v.cloned().unwrap_or(::serde_json::Value::Null),
                })
            }
        } else if let Some(x) = $v.and_then(|$v| $v.as_i64()) {
            if x >= $f::MIN.ceil() as i64 && x <= $f::MAX.floor() as i64 {
                Ok(())
            } else {
                Err(Error::$err {
                    got: $v.cloned().unwrap_or(::serde_json::Value::Null),
                })
            }
        } else if let Some(x) = $v.and_then(|$v| $v.as_u64()) {
            if x <= $f::MAX.floor() as u64 {
                Ok(())
            } else {
                Err(Error::$err {
                    got: $v.cloned().unwrap_or(::serde_json::Value::Null),
                })
            }
        } else {
            Err(Error::$err {
                got: $v.cloned().unwrap_or(::serde_json::Value::Null),
            })
        }
    };
}

#[cfg(test)]

mod test {
    use serde::Serialize;

    use super::*;
    use crate::TypeInfo;

    #[cfg(feature = "serde")]
    pub struct U8MutableValueVisitor<F, Error>
    where
        F: FnMut(&mut u8, &Type) -> Result<(), Error>,
    {
        f: F,
    }

    #[cfg(feature = "serde")]
    impl<F, Error> U8MutableValueVisitor<F, Error>
    where
        F: FnMut(&mut u8, &Type) -> Result<(), Error>,
    {
        pub fn new(f: F) -> Self {
            Self { f }
        }
    }

    impl<F, Error> MutableValueVisitor for U8MutableValueVisitor<F, Error>
    where
        F: FnMut(&mut u8, &Type) -> Result<(), Error>,
    {
        fn visit_u8(&mut self, val: &mut Value) -> Result<(), Error> {
            let mut num = val.as_u64().expect("value should be u8") as u8;
            let _ = (self.f)(&mut num, &u8::t());

            *val = serde_json::to_value(num).unwrap();

            Ok(())
        }
        type Error = Error;
    }

    #[test]
    fn test_walk_value_mutable_with_u8() {
        let num: u8 = 1;
        let mut num_value = serde_json::to_value(num).unwrap();

        let mut visited = 0;
        let mut visitor = U8MutableValueVisitor::new(|num: &mut u8, _t| {
            *num = 2;
            visited += 1;
            Ok::<(), ()>(())
        });
        walk_value_mut(&mut visitor, &mut u8::t(), &mut num_value).unwrap();
        assert_eq!(visited, 1);
        assert_eq!(num_value.as_u64().unwrap(), 2);
    }

    #[test]
    fn test_walk_fallible_type() {
        struct FallibleTypeVisitor {
            visited_array: bool,
            visited_u8: bool,
            visited_u16: bool,
            visited_fallible: bool,
            visited_optional: bool,
        }

        impl TypeVisitor for FallibleTypeVisitor {
            type Error = ();

            fn visit_u8(&mut self) -> Result<(), Self::Error> {
                self.visited_u8 = true;
                Ok(())
            }

            fn visit_u16(&mut self) -> Result<(), Self::Error> {
                self.visited_u16 = true;
                Ok(())
            }

            fn visit_array(&mut self, _arr: &Array) -> Result<(), Self::Error> {
                self.visited_array = true;
                Ok(())
            }

            fn visit_fallible(&mut self, _fall: &Fallible) -> Result<(), Self::Error> {
                self.visited_fallible = true;
                Ok(())
            }

            fn visit_optional(&mut self, _opt: &Optional) -> Result<(), Self::Error> {
                self.visited_optional = true;
                Ok(())
            }
        }

        let mut visitor = FallibleTypeVisitor {
            visited_fallible: false,
            visited_u8: false,
            visited_u16: false,
            visited_array: false,
            visited_optional: false,
        };
        walk_type(&mut visitor, &Option::<Result<u8, Vec<u16>>>::t()).unwrap();

        assert!(visitor.visited_u8);
        assert!(visitor.visited_u16);
        assert!(visitor.visited_array);
        assert!(visitor.visited_fallible);
        assert!(visitor.visited_optional);
    }

    #[test]
    fn test_walk_fallible_value() {
        struct FallibleValueVisitor {
            visited_array: bool,
            visited_u8: bool,
            visited_u16: bool,
            visited_fallible_ok: bool,
            visited_fallible_err: bool,
            visited_optional: bool,
        }

        impl ValueVisitor for FallibleValueVisitor {
            type Error = ();

            fn visit_u8(&mut self, _v: u8) -> Result<(), Self::Error> {
                self.visited_u8 = true;
                Ok(())
            }

            fn visit_u16(&mut self, _v: u16) -> Result<(), Self::Error> {
                self.visited_u16 = true;
                Ok(())
            }

            fn visit_array(&mut self, _arr: &Array, _v: &[Value]) -> Result<(), Self::Error> {
                self.visited_array = true;
                Ok(())
            }

            fn visit_fallible_ok(
                &mut self,
                _fall: &Fallible,
                _v: &Value,
            ) -> Result<(), Self::Error> {
                self.visited_fallible_ok = true;
                Ok(())
            }

            fn visit_fallible_err(
                &mut self,
                _fall: &Fallible,
                _v: &Value,
            ) -> Result<(), Self::Error> {
                self.visited_fallible_err = true;
                Ok(())
            }

            fn visit_optional(
                &mut self,
                _opt: &Optional,
                _v: Option<&Value>,
            ) -> Result<(), Self::Error> {
                self.visited_optional = true;
                Ok(())
            }
        }

        let mut visitor = FallibleValueVisitor {
            visited_u8: false,
            visited_u16: false,
            visited_array: false,
            visited_fallible_ok: false,
            visited_fallible_err: false,
            visited_optional: false,
        };

        walk_value(
            &mut visitor,
            &Option::<Result<u8, Vec<u16>>>::t(),
            &serde_json::Value::Null,
        )
        .unwrap();

        assert!(!visitor.visited_u8);
        assert!(!visitor.visited_u16);
        assert!(!visitor.visited_array);
        assert!(!visitor.visited_fallible_ok);
        assert!(!visitor.visited_fallible_err);
        assert!(visitor.visited_optional);

        let mut visitor = FallibleValueVisitor {
            visited_u8: false,
            visited_u16: false,
            visited_array: false,
            visited_fallible_ok: false,
            visited_fallible_err: false,
            visited_optional: false,
        };

        walk_value(
            &mut visitor,
            &Option::<Result<u8, Vec<u16>>>::t(),
            &serde_json::to_value(Some(Ok::<_, Vec<u16>>(1))).unwrap(),
        )
        .unwrap();

        assert!(visitor.visited_u8);
        assert!(!visitor.visited_u16);
        assert!(!visitor.visited_array);
        assert!(visitor.visited_fallible_ok);
        assert!(!visitor.visited_fallible_err);
        assert!(visitor.visited_optional);

        let mut visitor = FallibleValueVisitor {
            visited_u8: false,
            visited_u16: false,
            visited_array: false,
            visited_fallible_ok: false,
            visited_fallible_err: false,
            visited_optional: false,
        };

        walk_value(
            &mut visitor,
            &Option::<Result<u8, Vec<u16>>>::t(),
            &serde_json::to_value(Some(Err::<u8, _>(vec![1, 2, 3]))).unwrap(),
        )
        .unwrap();

        assert!(!visitor.visited_u8);
        assert!(visitor.visited_u16);
        assert!(visitor.visited_array);
        assert!(!visitor.visited_fallible_ok);
        assert!(visitor.visited_fallible_err);
        assert!(visitor.visited_optional);
    }

    #[test]
    fn test_walk_union() {
        #[derive(Serialize)]
        struct Baz {
            x: u8,
            y: String,
            z: Vec<u8>,
        }

        #[derive(Serialize)]
        enum Foo {
            Bar(Baz),
        }

        struct Visitor {
            visited_u8: usize,
        }

        impl ValueVisitor for Visitor {
            type Error = ();

            fn visit_u8(&mut self, _v: u8) -> Result<(), Self::Error> {
                self.visited_u8 += 1;
                Ok(())
            }
        }

        impl MutableValueVisitor for Visitor {
            type Error = ();

            fn visit_u8(&mut self, _v: &mut serde_json::Value) -> Result<(), Self::Error> {
                self.visited_u8 += 1;
                Ok(())
            }
        }

        let foo = Foo::Bar(Baz {
            x: 2,
            y: "y".to_string(),
            z: vec![3, 4, 5],
        });
        let mut val = serde_json::to_value(&foo).unwrap();

        let mut visitor = Visitor { visited_u8: 0 };
        let _ = walk_value_mut(
            &mut visitor,
            &mut Type::Union(Union::new(
                "Foo",
                [UnionVariant::with_fields(
                    "Bar",
                    Fields::unnamed([Type::Record(Record::new(
                        "Baz",
                        Fields::named([("x", u8::t()), ("y", String::t()), ("z", Vec::<u8>::t())]),
                    ))]),
                )],
            )),
            &mut val,
        );
        assert_eq!(visitor.visited_u8, 4);

        let mut visitor = Visitor { visited_u8: 0 };
        let _ = walk_value(
            &mut visitor,
            &Type::Union(Union::new(
                "Foo",
                [UnionVariant::with_fields(
                    "Bar",
                    Fields::unnamed([Type::Record(Record::new(
                        "Baz",
                        Fields::named([("x", u8::t()), ("y", String::t()), ("z", Vec::<u8>::t())]),
                    ))]),
                )],
            )),
            &val,
        );
        assert_eq!(visitor.visited_u8, 4);
    }
}
