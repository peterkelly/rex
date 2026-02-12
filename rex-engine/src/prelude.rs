//! Prelude injection helpers for Rex.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rex_ast::expr::{Symbol, intern, sym, sym_eq};
use rex_ts::{Scheme, Type, TypeKind, Types, unify};
use uuid::Uuid;

use crate::engine::{apply, binary_arg_types, expect_bool, option_value};
use crate::value::{Heap, list_from_vec, list_to_vec};
use crate::virtual_export_name;
use crate::{Engine, EngineError, FromValue, OverloadedFn, Value};

pub(crate) fn list_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "List")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "List a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn array_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Array")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Array a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn dict_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Dict")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Dict a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn option_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Option")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Option a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn result_types(typ: &Type, name: &str) -> Result<(Type, Type), EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, ok) => match head.as_ref() {
            TypeKind::App(head, err) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Result")) => {
                Ok((ok.clone(), err.clone()))
            }
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "Result a e".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Result a e".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn resolve_binary_op(
    engine: &Engine,
    name: &str,
    elem_ty: &Type,
) -> Result<Value, EngineError> {
    let op_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
    engine.resolve_global_value(&sym(name), &op_ty)
}

pub(crate) fn len_value_for_type(
    heap: &Heap,
    elem_ty: &Type,
    len: usize,
    name: &str,
) -> Result<Value, EngineError> {
    match elem_ty.as_ref() {
        TypeKind::Con(c) if sym_eq(&c.name, "f32") => heap.alloc_f32(len as f32)?.get_value(heap),
        TypeKind::Con(c) if sym_eq(&c.name, "f64") => heap.alloc_f64(len as f64)?.get_value(heap),
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "f32 or f64".into(),
            got: elem_ty.to_string(),
        }),
    }
}

pub(crate) fn expect_array(value: &Value, name: &str) -> Result<Vec<Value>, EngineError> {
    match value {
        Value::Array(xs) => Ok(xs.clone()),
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "array".into(),
            got: value.type_name().into(),
        }),
    }
}

pub(crate) fn option_from_value(heap: &Heap, value: Option<Value>) -> Result<Value, EngineError> {
    match value {
        Some(v) => heap.alloc_adt(sym("Some"), vec![v])?.get_value(heap),
        None => heap.alloc_adt(sym("None"), vec![])?.get_value(heap),
    }
}

pub(crate) fn result_value(value: &Value) -> Result<Result<Value, Value>, EngineError> {
    match value {
        Value::Adt(name, args) if sym_eq(name, "Ok") && args.len() == 1 => Ok(Ok(args[0].clone())),
        Value::Adt(name, args) if sym_eq(name, "Err") && args.len() == 1 => {
            Ok(Err(args[0].clone()))
        }
        _ => Err(EngineError::NativeType {
            name: sym("result"),
            expected: "Result".into(),
            got: value.type_name().into(),
        }),
    }
}

pub(crate) fn result_from_value(
    heap: &Heap,
    value: Result<Value, Value>,
) -> Result<Value, EngineError> {
    match value {
        Ok(v) => heap.alloc_adt(sym("Ok"), vec![v])?.get_value(heap),
        Err(v) => heap.alloc_adt(sym("Err"), vec![v])?.get_value(heap),
    }
}

pub(crate) fn split_fun_chain(
    name: &str,
    typ: &Type,
    count: usize,
) -> Result<(Vec<Type>, Type), EngineError> {
    let mut args = Vec::with_capacity(count);
    let mut cur = typ.clone();
    for _ in 0..count {
        let (arg, rest) = match cur.as_ref() {
            TypeKind::Fun(arg, rest) => (arg.clone(), rest.clone()),
            _ => {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: format!("function of arity {}", count),
                    got: typ.to_string(),
                });
            }
        };
        args.push(arg);
        cur = rest;
    }
    Ok((args, cur))
}

pub(crate) fn tuple_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::Tuple(elems) if !elems.is_empty() => {
            let first = elems[0].clone();
            for elem in elems.iter().skip(1) {
                if *elem != first {
                    return Err(EngineError::NativeType {
                        name: sym(name),
                        expected: first.to_string(),
                        got: elem.to_string(),
                    });
                }
            }
            Ok(first)
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "tuple".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn map_values(
    engine: &Engine,
    func: Value,
    func_ty: &Type,
    elem_ty: &Type,
    values: impl IntoIterator<Item = Value>,
) -> Result<Vec<Value>, EngineError> {
    values
        .into_iter()
        .map(|value| apply(engine, func.clone(), value, Some(func_ty), Some(elem_ty)))
        .collect()
}

pub(crate) fn filter_values(
    engine: &Engine,
    name: &'static str,
    pred: Value,
    pred_ty: &Type,
    elem_ty: &Type,
    values: impl IntoIterator<Item = Value>,
) -> Result<Vec<Value>, EngineError> {
    let mut out = Vec::new();
    for value in values {
        let keep = apply(
            engine,
            pred.clone(),
            value.clone(),
            Some(pred_ty),
            Some(elem_ty),
        )?;
        if expect_bool(&keep, name)? {
            out.push(value);
        }
    }
    Ok(out)
}

pub(crate) fn filter_map_values(
    engine: &Engine,
    func: Value,
    func_ty: &Type,
    elem_ty: &Type,
    values: impl IntoIterator<Item = Value>,
) -> Result<Vec<Value>, EngineError> {
    let mut out = Vec::new();
    for value in values {
        let mapped = apply(engine, func.clone(), value, Some(func_ty), Some(elem_ty))?;
        if let Some(v) = option_value(&mapped)? {
            out.push(v);
        }
    }
    Ok(out)
}

pub(crate) fn flat_map_values(
    engine: &Engine,
    func: Value,
    func_ty: &Type,
    elem_ty: &Type,
    values: impl IntoIterator<Item = Value>,
    mut extract: impl FnMut(&Value) -> Result<Vec<Value>, EngineError>,
) -> Result<Vec<Value>, EngineError> {
    let mut out = Vec::new();
    for value in values {
        let mapped = apply(engine, func.clone(), value, Some(func_ty), Some(elem_ty))?;
        out.extend(extract(&mapped)?);
    }
    Ok(out)
}

pub(crate) fn foldl_values(
    engine: &Engine,
    func: Value,
    func_ty: &Type,
    acc_ty: &Type,
    elem_ty: &Type,
    mut acc: Value,
    values: impl IntoIterator<Item = Value>,
) -> Result<Value, EngineError> {
    let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
    for value in values {
        let step = apply(engine, func.clone(), acc, Some(func_ty), Some(acc_ty))?;
        acc = apply(engine, step, value, Some(&step_ty), Some(elem_ty))?;
    }
    Ok(acc)
}

pub(crate) fn foldr_values(
    engine: &Engine,
    func: Value,
    func_ty: &Type,
    acc_ty: &Type,
    elem_ty: &Type,
    mut acc: Value,
    values: Vec<Value>,
) -> Result<Value, EngineError> {
    let step_ty = Type::fun(acc_ty.clone(), acc_ty.clone());
    for value in values.into_iter().rev() {
        let step = apply(engine, func.clone(), value, Some(func_ty), Some(elem_ty))?;
        acc = apply(engine, step, acc, Some(&step_ty), Some(acc_ty))?;
    }
    Ok(acc)
}

pub(crate) fn extremum_by_type(
    name: &'static str,
    elem_ty: &Type,
    values: Vec<Value>,
    choose: std::cmp::Ordering,
) -> Result<Value, EngineError> {
    let name = sym(name);
    let mut values = values.into_iter();
    let mut best = values.next().ok_or(EngineError::EmptySequence)?;
    for value in values {
        let ord = cmp_value_by_type(&name, elem_ty, &value, &best)?;
        if ord == choose {
            best = value;
        }
    }
    Ok(best)
}

pub(crate) fn checked_index(name: Symbol, index: i32, len: usize) -> Result<usize, EngineError> {
    if index < 0 {
        return Err(EngineError::IndexOutOfBounds { name, index, len });
    }
    let index_usize = index as usize;
    if index_usize >= len {
        return Err(EngineError::IndexOutOfBounds { name, index, len });
    }
    Ok(index_usize)
}

pub(crate) fn zip_tuple2(
    heap: &Heap,
    xs: Vec<Value>,
    ys: Vec<Value>,
) -> Result<Vec<Value>, EngineError> {
    xs.into_iter()
        .zip(ys)
        .map(|(x, y)| heap.alloc_tuple(vec![x, y])?.get_value(heap))
        .collect()
}

pub(crate) fn unzip_tuple2(
    name: Symbol,
    pairs: Vec<Value>,
) -> Result<(Vec<Value>, Vec<Value>), EngineError> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for value in pairs {
        match value {
            Value::Tuple(elems) => {
                let len = elems.len();
                let Ok([a, b]) = <[Value; 2]>::try_from(elems) else {
                    return Err(EngineError::NativeType {
                        name,
                        expected: "tuple2".into(),
                        got: format!("tuple{len}"),
                    });
                };
                left.push(a);
                right.push(b);
            }
            other => {
                return Err(EngineError::NativeType {
                    name,
                    expected: "tuple2".into(),
                    got: other.type_name().into(),
                });
            }
        }
    }
    Ok((left, right))
}

pub(crate) fn as_nonneg_usize(n: i32) -> usize {
    if n <= 0 { 0 } else { n as usize }
}

fn cmp_value_by_type(
    op_name: &Symbol,
    typ: &Type,
    lhs: &Value,
    rhs: &Value,
) -> Result<std::cmp::Ordering, EngineError> {
    match typ.as_ref() {
        TypeKind::Con(tc) => match tc.name.as_ref() {
            "u8" => match (lhs, rhs) {
                (Value::U8(a), Value::U8(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u16" => match (lhs, rhs) {
                (Value::U16(a), Value::U16(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u32" => match (lhs, rhs) {
                (Value::U32(a), Value::U32(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u64" => match (lhs, rhs) {
                (Value::U64(a), Value::U64(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i8" => match (lhs, rhs) {
                (Value::I8(a), Value::I8(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i16" => match (lhs, rhs) {
                (Value::I16(a), Value::I16(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i32" => match (lhs, rhs) {
                (Value::I32(a), Value::I32(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i64" => match (lhs, rhs) {
                (Value::I64(a), Value::I64(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "f32" => match (lhs, rhs) {
                (Value::F32(a), Value::F32(b)) => {
                    a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                        name: op_name.clone(),
                        expected: tc.name.to_string(),
                        got: "nan".into(),
                    })
                }
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "f64" => match (lhs, rhs) {
                (Value::F64(a), Value::F64(b)) => {
                    a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                        name: op_name.clone(),
                        expected: tc.name.to_string(),
                        got: "nan".into(),
                    })
                }
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "string" => match (lhs, rhs) {
                (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "uuid" => match (lhs, rhs) {
                (Value::Uuid(a), Value::Uuid(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "datetime" => match (lhs, rhs) {
                (Value::DateTime(a), Value::DateTime(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            _ => Err(EngineError::NativeType {
                name: op_name.clone(),
                expected: tc.name.to_string(),
                got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
            }),
        },
        _ => Err(EngineError::NativeType {
            name: op_name.clone(),
            expected: typ.to_string(),
            got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
        }),
    }
}

pub(crate) fn inject_prelude_adts(engine: &mut Engine) -> Result<(), EngineError> {
    let mut list_adt = engine.adt_decl("List", &["a"]);
    let a_name = sym("a");
    let a = list_adt
        .param_type(&a_name)
        .ok_or_else(|| EngineError::UnknownType(sym("List")))?;
    let list_a = list_adt.result_type();
    list_adt.add_variant(sym("Empty"), vec![]);
    list_adt.add_variant(sym("Cons"), vec![a, list_a]);
    engine.inject_adt(list_adt)?;

    let mut option_adt = engine.adt_decl("Option", &["t"]);
    let t_name = sym("t");
    let t = option_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(sym("Option")))?;
    option_adt.add_variant(sym("Some"), vec![t]);
    option_adt.add_variant(sym("None"), vec![]);
    engine.inject_adt(option_adt)?;

    let mut result_adt = engine.adt_decl("Result", &["e", "t"]);
    let e_name = sym("e");
    let t_name = sym("t");
    let e = result_adt
        .param_type(&e_name)
        .ok_or_else(|| EngineError::UnknownType(sym("Result")))?;
    let t = result_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(sym("Result")))?;
    result_adt.add_variant(sym("Err"), vec![e]);
    result_adt.add_variant(sym("Ok"), vec![t]);
    engine.inject_adt(result_adt)?;
    Ok(())
}

pub(crate) fn inject_equality_ops(engine: &mut Engine) -> Result<(), EngineError> {
    // Equality primitives are monomorphic overloads (same name, different
    // concrete types), matching the numeric `prim_add` style.
    engine.inject_fn2("prim_eq", |a: bool, b: bool| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: bool, b: bool| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: u8, b: u8| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u8, b: u8| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: u16, b: u16| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u16, b: u16| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: u32, b: u32| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u32, b: u32| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: u64, b: u64| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u64, b: u64| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: i8, b: i8| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i8, b: i8| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: i16, b: i16| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i16, b: i16| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: i32, b: i32| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i32, b: i32| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: i64, b: i64| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i64, b: i64| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: f32, b: f32| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: f32, b: f32| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: f64, b: f64| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: f64, b: f64| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: String, b: String| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: String, b: String| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: Uuid, b: Uuid| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: Uuid, b: Uuid| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: DateTime<Utc>, b: DateTime<Utc>| -> bool {
        a == b
    })?;
    engine.inject_fn2("prim_ne", |a: DateTime<Utc>, b: DateTime<Utc>| -> bool {
        a != b
    })?;

    // Array equality must respect `Eq a`. We can't express the loop without a
    // primitive, but we *can* express the element comparison: the primitive
    // calls `(==)` on each pair.
    {
        let a_tv = engine.fresh_type_var(Some(sym("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::app(Type::con("Array", 1), a);
        let bool_ty = Type::con("bool", 0);
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_a.clone(), bool_ty.clone())),
        );
        engine.inject_native_scheme_typed(
            "prim_array_eq",
            scheme.clone(),
            2,
            |engine, call_type, args| {
                let (lhs_ty, rhs_ty) = binary_arg_types("prim_array_eq", call_type)?;
                let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
                    name: sym("prim_array_eq"),
                    expected: lhs_ty.to_string(),
                    got: rhs_ty.to_string(),
                })?;
                let array_ty = lhs_ty.apply(&subst);
                let elem_ty = array_elem_type(&array_ty, "prim_array_eq")?;
                let xs = expect_array(&args[0], "prim_array_eq")?;
                let ys = expect_array(&args[1], "prim_array_eq")?;
                if xs.len() != ys.len() {
                    return engine.heap().alloc_bool(false)?.get_value(engine.heap());
                }

                let bool_ty = Type::con("bool", 0);
                let eq_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), bool_ty.clone()));
                let step_ty = Type::fun(elem_ty.clone(), bool_ty);
                for (x, y) in xs.iter().zip(ys.iter()) {
                    let (name, typ, applied, applied_types) =
                        OverloadedFn::new(sym("=="), eq_ty.clone()).into_parts();
                    let f = engine
                        .heap()
                        .alloc_overloaded(name, typ, applied, applied_types)?
                        .get_value(engine.heap())?;
                    let f = apply(engine, f, x.clone(), Some(&eq_ty), Some(&elem_ty))?;
                    let r = apply(engine, f, y.clone(), Some(&step_ty), Some(&elem_ty))?;
                    if !expect_bool(&r, "prim_array_eq")? {
                        return engine.heap().alloc_bool(false)?.get_value(engine.heap());
                    }
                }
                engine.heap().alloc_bool(true)?.get_value(engine.heap())
            },
        )?;

        let scheme = Scheme::new(
            vec![a_tv],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_a, bool_ty.clone())),
        );
        engine.inject_native_scheme_typed(
            "prim_array_ne",
            scheme,
            2,
            |engine, call_type, args| {
                let eq = engine.call_native_impl_sync("prim_array_eq", call_type, args)?;
                engine
                    .heap()
                    .alloc_bool(!expect_bool(&eq, "prim_array_ne")?)?
                    .get_value(engine.heap())
            },
        )?;
    }

    Ok(())
}

pub(crate) fn inject_order_ops(engine: &mut Engine) -> Result<(), EngineError> {
    fn cmp_to_i32(ord: std::cmp::Ordering) -> i32 {
        match ord {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    // Integer and string comparisons can be injected as direct typed natives,
    // with no runtime type switching.
    engine.inject_fn2("prim_lt", |a: u8, b: u8| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u8, b: u8| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u8, b: u8| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u8, b: u8| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u8, b: u8| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: u16, b: u16| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u16, b: u16| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u16, b: u16| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u16, b: u16| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u16, b: u16| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.inject_fn2("prim_lt", |a: u32, b: u32| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u32, b: u32| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u32, b: u32| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u32, b: u32| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u32, b: u32| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.inject_fn2("prim_lt", |a: u64, b: u64| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u64, b: u64| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u64, b: u64| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u64, b: u64| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u64, b: u64| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.inject_fn2("prim_lt", |a: i8, b: i8| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i8, b: i8| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i8, b: i8| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i8, b: i8| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i8, b: i8| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: i16, b: i16| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i16, b: i16| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i16, b: i16| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i16, b: i16| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i16, b: i16| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.inject_fn2("prim_lt", |a: i32, b: i32| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i32, b: i32| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i32, b: i32| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i32, b: i32| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i32, b: i32| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.inject_fn2("prim_lt", |a: i64, b: i64| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i64, b: i64| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i64, b: i64| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i64, b: i64| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i64, b: i64| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.inject_fn2("prim_lt", |a: String, b: String| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: String, b: String| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: String, b: String| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: String, b: String| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: String, b: String| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    // Floats: preserve the existing “NaN is a type error” semantics.
    let bool_ty = Type::con("bool", 0);
    let i32_ty = Type::con("i32", 0);

    let f32_ty = Type::con("f32", 0);
    let f32_bool = Scheme::new(
        vec![],
        vec![],
        Type::fun(f32_ty.clone(), Type::fun(f32_ty.clone(), bool_ty.clone())),
    );
    let f32_cmp = Scheme::new(
        vec![],
        vec![],
        Type::fun(f32_ty.clone(), Type::fun(f32_ty.clone(), i32_ty.clone())),
    );
    for (name, pred) in [
        (
            "prim_lt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_le",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_gt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_ge",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
    ] {
        let scheme = f32_bool.clone();
        engine.inject_native_scheme_typed(name, scheme, 2, move |engine, _call_type, args| {
            let Value::F32(a) = &args[0] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f32".into(),
                    got: args[0].type_name().into(),
                });
            };
            let Value::F32(b) = &args[1] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f32".into(),
                    got: args[1].type_name().into(),
                });
            };
            let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                name: sym(name),
                expected: "f32".into(),
                got: "nan".into(),
            })?;
            engine
                .heap()
                .alloc_bool(pred(ord))?
                .get_value(engine.heap())
        })?;
    }
    engine.inject_native_scheme_typed("prim_cmp", f32_cmp, 2, |engine, _call_type, args| {
        let Value::F32(a) = &args[0] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f32".into(),
                got: args[0].type_name().into(),
            });
        };
        let Value::F32(b) = &args[1] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f32".into(),
                got: args[1].type_name().into(),
            });
        };
        let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
            name: sym("prim_cmp"),
            expected: "f32".into(),
            got: "nan".into(),
        })?;
        engine
            .heap()
            .alloc_i32(cmp_to_i32(ord))?
            .get_value(engine.heap())
    })?;

    let f64_ty = Type::con("f64", 0);
    let f64_bool = Scheme::new(
        vec![],
        vec![],
        Type::fun(f64_ty.clone(), Type::fun(f64_ty.clone(), bool_ty.clone())),
    );
    let f64_cmp = Scheme::new(
        vec![],
        vec![],
        Type::fun(f64_ty.clone(), Type::fun(f64_ty.clone(), i32_ty)),
    );
    for (name, pred) in [
        (
            "prim_lt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_le",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_gt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_ge",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
    ] {
        let scheme = f64_bool.clone();
        engine.inject_native_scheme_typed(name, scheme, 2, move |engine, _call_type, args| {
            let Value::F64(a) = &args[0] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f64".into(),
                    got: args[0].type_name().into(),
                });
            };
            let Value::F64(b) = &args[1] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f64".into(),
                    got: args[1].type_name().into(),
                });
            };
            let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                name: sym(name),
                expected: "f64".into(),
                got: "nan".into(),
            })?;
            engine
                .heap()
                .alloc_bool(pred(ord))?
                .get_value(engine.heap())
        })?;
    }
    engine.inject_native_scheme_typed("prim_cmp", f64_cmp, 2, |engine, _call_type, args| {
        let Value::F64(a) = &args[0] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f64".into(),
                got: args[0].type_name().into(),
            });
        };
        let Value::F64(b) = &args[1] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f64".into(),
                got: args[1].type_name().into(),
            });
        };
        let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
            name: sym("prim_cmp"),
            expected: "f64".into(),
            got: "nan".into(),
        })?;
        engine
            .heap()
            .alloc_i32(cmp_to_i32(ord))?
            .get_value(engine.heap())
    })?;

    Ok(())
}

pub(crate) fn inject_pretty_ops(engine: &mut Engine) -> Result<(), EngineError> {
    engine.inject_fn1("prim_pretty", |x: bool| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u8| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u16| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u32| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u64| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i8| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i16| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i32| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i64| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: f32| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: f64| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: String| -> String { x })?;
    engine.inject_fn1("prim_pretty", |x: Uuid| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: DateTime<Utc>| -> String {
        x.to_string()
    })?;
    Ok(())
}

pub(crate) fn inject_boolean_ops(engine: &mut Engine) -> Result<(), EngineError> {
    engine.inject_fn2("(&&)", |a: bool, b: bool| -> bool { a && b })?;
    engine.inject_fn2("(||)", |a: bool, b: bool| -> bool { a || b })?;
    Ok(())
}

pub(crate) fn inject_numeric_ops(engine: &mut Engine) -> Result<(), EngineError> {
    // Additive identity
    engine.inject_value("prim_zero", String::new())?;
    engine.inject_value("prim_zero", 0u8)?;
    engine.inject_value("prim_zero", 0u16)?;
    engine.inject_value("prim_zero", 0u32)?;
    engine.inject_value("prim_zero", 0u64)?;
    engine.inject_value("prim_zero", 0i8)?;
    engine.inject_value("prim_zero", 0i16)?;
    engine.inject_value("prim_zero", 0i32)?;
    engine.inject_value("prim_zero", 0i64)?;
    engine.inject_value("prim_zero", 0.0f32)?;
    engine.inject_value("prim_zero", 0.0f64)?;

    // Multiplicative identity
    engine.inject_value("prim_one", 1u8)?;
    engine.inject_value("prim_one", 1u16)?;
    engine.inject_value("prim_one", 1u32)?;
    engine.inject_value("prim_one", 1u64)?;
    engine.inject_value("prim_one", 1i8)?;
    engine.inject_value("prim_one", 1i16)?;
    engine.inject_value("prim_one", 1i32)?;
    engine.inject_value("prim_one", 1i64)?;
    engine.inject_value("prim_one", 1.0f32)?;
    engine.inject_value("prim_one", 1.0f64)?;

    // Addition
    engine.inject_fn2("prim_add", |a: u8, b: u8| -> u8 { a + b })?;
    engine.inject_fn2("prim_add", |a: u16, b: u16| -> u16 { a + b })?;
    engine.inject_fn2("prim_add", |a: u32, b: u32| -> u32 { a + b })?;
    engine.inject_fn2("prim_add", |a: u64, b: u64| -> u64 { a + b })?;
    engine.inject_fn2("prim_add", |a: i8, b: i8| -> i8 { a + b })?;
    engine.inject_fn2("prim_add", |a: i16, b: i16| -> i16 { a + b })?;
    engine.inject_fn2("prim_add", |a: i32, b: i32| -> i32 { a + b })?;
    engine.inject_fn2("prim_add", |a: i64, b: i64| -> i64 { a + b })?;
    engine.inject_fn2("prim_add", |a: f32, b: f32| -> f32 { a + b })?;
    engine.inject_fn2("prim_add", |a: f64, b: f64| -> f64 { a + b })?;
    engine.inject_fn2("prim_add", |a: String, b: String| -> String {
        format!("{}{}", a, b)
    })?;

    // Subtraction and negation
    engine.inject_fn2("prim_sub", |a: i8, b: i8| -> i8 { a - b })?;
    engine.inject_fn2("prim_sub", |a: i16, b: i16| -> i16 { a - b })?;
    engine.inject_fn2("prim_sub", |a: i32, b: i32| -> i32 { a - b })?;
    engine.inject_fn2("prim_sub", |a: i64, b: i64| -> i64 { a - b })?;
    engine.inject_fn2("prim_sub", |a: f32, b: f32| -> f32 { a - b })?;
    engine.inject_fn2("prim_sub", |a: f64, b: f64| -> f64 { a - b })?;
    engine.inject_fn1("prim_negate", |a: i8| -> i8 { -a })?;
    engine.inject_fn1("prim_negate", |a: i16| -> i16 { -a })?;
    engine.inject_fn1("prim_negate", |a: i32| -> i32 { -a })?;
    engine.inject_fn1("prim_negate", |a: i64| -> i64 { -a })?;
    engine.inject_fn1("prim_negate", |a: f32| -> f32 { -a })?;
    engine.inject_fn1("prim_negate", |a: f64| -> f64 { -a })?;

    // Multiplication and division
    engine.inject_fn2("prim_mul", |a: u8, b: u8| -> u8 { a * b })?;
    engine.inject_fn2("prim_mul", |a: u16, b: u16| -> u16 { a * b })?;
    engine.inject_fn2("prim_mul", |a: u32, b: u32| -> u32 { a * b })?;
    engine.inject_fn2("prim_mul", |a: u64, b: u64| -> u64 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i8, b: i8| -> i8 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i16, b: i16| -> i16 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i32, b: i32| -> i32 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i64, b: i64| -> i64 { a * b })?;
    engine.inject_fn2("prim_mul", |a: f32, b: f32| -> f32 { a * b })?;
    engine.inject_fn2("prim_mul", |a: f64, b: f64| -> f64 { a * b })?;
    engine.inject_fn2("prim_div", |a: f32, b: f32| -> f32 { a / b })?;
    engine.inject_fn2("prim_div", |a: f64, b: f64| -> f64 { a / b })?;

    // Remainder
    engine.inject_fn2("prim_mod", |a: u8, b: u8| -> u8 { a % b })?;
    engine.inject_fn2("prim_mod", |a: u16, b: u16| -> u16 { a % b })?;
    engine.inject_fn2("prim_mod", |a: u32, b: u32| -> u32 { a % b })?;
    engine.inject_fn2("prim_mod", |a: u64, b: u64| -> u64 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i8, b: i8| -> i8 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i16, b: i16| -> i16 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i32, b: i32| -> i32 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i64, b: i64| -> i64 { a % b })?;

    // Numeric conversions (used by `std.json`).
    engine.inject_fn1("prim_to_f64", |x: u8| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: u16| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: u32| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: u64| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: i8| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: i16| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: i32| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: i64| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: f32| -> f64 { x as f64 })?;
    engine.inject_fn1("prim_to_f64", |x: f64| -> f64 { x })?;

    // f64 -> Option <number> conversions (used by `std.json`).
    // - reject NaN/±inf
    // - for integer types: require integral `x` (fract == 0) and in range
    {
        let f64_ty = Type::con("f64", 0);

        let inject = |engine: &mut Engine,
                      name: &'static str,
                      dst_ty: Type,
                      conv: fn(&Heap, f64) -> Result<Option<Value>, EngineError>|
         -> Result<(), EngineError> {
            let scheme = Scheme::new(
                vec![],
                vec![],
                Type::fun(f64_ty.clone(), Type::option(dst_ty)),
            );
            engine.inject_native_scheme_typed(name, scheme, 1, move |engine, _t, args| {
                let Value::F64(x) = args[0] else {
                    return Err(EngineError::NativeType {
                        name: sym(name),
                        expected: "f64".into(),
                        got: args[0].type_name().into(),
                    });
                };
                let converted = conv(engine.heap(), x)?;
                option_from_value(engine.heap(), converted)
            })
        };

        inject(engine, "prim_f64_to_u8", Type::con("u8", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= u8::MIN as f64 && x <= u8::MAX as f64 {
                Ok(Some(heap.alloc_u8(x as u8)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_u16", Type::con("u16", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= u16::MIN as f64 && x <= u16::MAX as f64 {
                Ok(Some(heap.alloc_u16(x as u16)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_u32", Type::con("u32", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= u32::MIN as f64 && x <= u32::MAX as f64 {
                Ok(Some(heap.alloc_u32(x as u32)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_u64", Type::con("u64", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= u64::MIN as f64 && x <= u64::MAX as f64 {
                Ok(Some(heap.alloc_u64(x as u64)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_i8", Type::con("i8", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= i8::MIN as f64 && x <= i8::MAX as f64 {
                Ok(Some(heap.alloc_i8(x as i8)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_i16", Type::con("i16", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= i16::MIN as f64 && x <= i16::MAX as f64 {
                Ok(Some(heap.alloc_i16(x as i16)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_i32", Type::con("i32", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= i32::MIN as f64 && x <= i32::MAX as f64 {
                Ok(Some(heap.alloc_i32(x as i32)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_i64", Type::con("i64", 0), |heap, x| {
            if x.is_finite() && x.fract() == 0.0 && x >= i64::MIN as f64 && x <= i64::MAX as f64 {
                Ok(Some(heap.alloc_i64(x as i64)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
        inject(engine, "prim_f64_to_f32", Type::con("f32", 0), |heap, x| {
            if x.is_finite() && x >= f32::MIN as f64 && x <= f32::MAX as f64 {
                Ok(Some(heap.alloc_f32(x as f32)?.get_value(heap)?))
            } else {
                Ok(None)
            }
        })?;
    }

    Ok(())
}

pub(crate) fn inject_json_primops(engine: &mut Engine) -> Result<(), EngineError> {
    // List -> Array conversion.
    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let array_a = Type::array(a);
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(list_a, array_a));
        engine.inject_native_scheme_typed(
            "prim_array_from_list",
            scheme,
            1,
            |engine, _call_type, args| {
                let values = list_to_vec(&args[0], "prim_array_from_list")?;
                engine.heap().alloc_array(values)?.get_value(engine.heap())
            },
        )?;
    }

    // Dict mapping and traversal helpers (used by `std.json`).
    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let dict_a = Type::dict(a.clone());
        let dict_b = Type::dict(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(dict_a.clone(), dict_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_dict_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_dict_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let dict_ty = arg_tys[1].clone();
                let elem_ty = dict_elem_type(&dict_ty, "prim_dict_map")?;
                let Value::Dict(map) = &args[1] else {
                    return Err(EngineError::NativeType {
                        name: sym("prim_dict_map"),
                        expected: "dict".into(),
                        got: args[1].type_name().into(),
                    });
                };
                let mut out: BTreeMap<Symbol, Value> = BTreeMap::new();
                for (k, v) in map {
                    let mapped = apply(
                        engine,
                        args[0].clone(),
                        v.clone(),
                        Some(&func_ty),
                        Some(&elem_ty),
                    )?;
                    out.insert(k.clone(), mapped);
                }
                engine.heap().alloc_dict(out)?.get_value(engine.heap())
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let e_tv = engine.fresh_type_var(Some("e".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let e = Type::var(e_tv.clone());
        let dict_a = Type::dict(a.clone());
        let dict_b = Type::dict(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv, e_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::result(b.clone(), e.clone())),
                Type::fun(dict_a.clone(), Type::result(dict_b, e.clone())),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_dict_traverse_result",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) =
                    split_fun_chain("prim_dict_traverse_result", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let dict_ty = arg_tys[1].clone();
                let elem_ty = dict_elem_type(&dict_ty, "prim_dict_traverse_result")?;
                let Value::Dict(map) = &args[1] else {
                    return Err(EngineError::NativeType {
                        name: sym("prim_dict_traverse_result"),
                        expected: "dict".into(),
                        got: args[1].type_name().into(),
                    });
                };

                let mut out: BTreeMap<Symbol, Value> = BTreeMap::new();
                for (k, v) in map {
                    let mapped = apply(
                        engine,
                        args[0].clone(),
                        v.clone(),
                        Some(&func_ty),
                        Some(&elem_ty),
                    )?;
                    match result_value(&mapped)? {
                        Ok(ok) => {
                            out.insert(k.clone(), ok);
                        }
                        Err(err) => return result_from_value(engine.heap(), Err(err)),
                    }
                }

                let dict = engine.heap().alloc_dict(out)?.get_value(engine.heap())?;
                result_from_value(engine.heap(), Ok(dict))
            },
        )?;
    }

    // Parsing helpers used by `std.json` instances.
    {
        let string_ty = Type::con("string", 0);
        let uuid_ty = Type::con("uuid", 0);
        let scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(string_ty.clone(), Type::option(uuid_ty)),
        );
        engine.inject_native_scheme_typed(
            "prim_parse_uuid",
            scheme,
            1,
            |engine, _call_type, args| {
                let Value::String(s) = &args[0] else {
                    return Err(EngineError::NativeType {
                        name: sym("prim_parse_uuid"),
                        expected: "string".into(),
                        got: args[0].type_name().into(),
                    });
                };
                let parsed = Uuid::parse_str(s)
                    .ok()
                    .map(|uuid| {
                        engine
                            .heap()
                            .alloc_uuid(uuid)
                            .and_then(|ptr| ptr.get_value(engine.heap()))
                    })
                    .transpose()?;
                option_from_value(engine.heap(), parsed)
            },
        )?;
    }

    {
        let string_ty = Type::con("string", 0);
        let dt_ty = Type::con("datetime", 0);
        let scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(string_ty.clone(), Type::option(dt_ty)),
        );
        engine.inject_native_scheme_typed(
            "prim_parse_datetime",
            scheme,
            1,
            |engine, _call_type, args| {
                let Value::String(s) = &args[0] else {
                    return Err(EngineError::NativeType {
                        name: sym("prim_parse_datetime"),
                        expected: "string".into(),
                        got: args[0].type_name().into(),
                    });
                };
                let parsed = DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
                    .map(|dt| {
                        engine
                            .heap()
                            .alloc_datetime(dt)
                            .and_then(|ptr| ptr.get_value(engine.heap()))
                    })
                    .transpose()?;
                option_from_value(engine.heap(), parsed)
            },
        )?;
    }

    // prim_json_stringify : a -> string
    //
    // Used by `std.json` to implement `Pretty Value` (JSON-encoded string).
    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let string_ty = Type::con("string", 0);
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(a, string_ty));

        #[derive(Clone)]
        struct Tags {
            null: Symbol,
            bool_: Symbol,
            string: Symbol,
            number: Symbol,
            array: Symbol,
            object: Symbol,
        }

        let tags = Tags {
            null: sym(&virtual_export_name("std.json", "Null")),
            bool_: sym(&virtual_export_name("std.json", "Bool")),
            string: sym(&virtual_export_name("std.json", "String")),
            number: sym(&virtual_export_name("std.json", "Number")),
            array: sym(&virtual_export_name("std.json", "Array")),
            object: sym(&virtual_export_name("std.json", "Object")),
        };

        fn to_serde_json(v: &Value, tags: &Tags) -> Option<serde_json::Value> {
            match v {
                Value::Adt(tag, _) if tag == &tags.null => Some(serde_json::Value::Null),
                Value::Adt(tag, args) if tag == &tags.bool_ => match args.as_slice() {
                    [Value::Bool(b)] => Some(serde_json::Value::Bool(*b)),
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.string => match args.as_slice() {
                    [Value::String(s)] => Some(serde_json::Value::String(s.clone())),
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.number => match args.as_slice() {
                    [Value::F64(n)] => serde_json::Number::from_f64(*n)
                        .map(serde_json::Value::Number)
                        .or(Some(serde_json::Value::Null)),
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.array => match args.as_slice() {
                    [Value::Array(xs)] => {
                        let mut out = Vec::with_capacity(xs.len());
                        for x in xs {
                            out.push(to_serde_json(x, tags)?);
                        }
                        Some(serde_json::Value::Array(out))
                    }
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.object => match args.as_slice() {
                    [Value::Dict(map)] => {
                        let mut out = serde_json::Map::with_capacity(map.len());
                        for (k, v) in map {
                            out.insert(k.as_ref().to_string(), to_serde_json(v, tags)?);
                        }
                        Some(serde_json::Value::Object(out))
                    }
                    _ => None,
                },
                _ => None,
            }
        }

        engine.inject_native_scheme_typed(
            "prim_json_stringify",
            scheme,
            1,
            move |engine, _call_type, args| {
                let Some(v) = args.first() else {
                    return Err(EngineError::Internal(
                        "prim_json_stringify expected 1 argument".into(),
                    ));
                };
                let Some(json) = to_serde_json(v, &tags) else {
                    return engine
                        .heap()
                        .alloc_string("<non-std.json.Value>".into())?
                        .get_value(engine.heap());
                };
                engine
                    .heap()
                    .alloc_string(json.to_string())?
                    .get_value(engine.heap())
            },
        )?;
    }

    // prim_json_parse : string -> Result a string
    //
    // This returns `Ok <std.json.Value>` when `a` is instantiated to the
    // qualified `std.json.Value` type. It's a primop, so we keep it minimal and
    // let `std.json.parse/from_string` wrap the string error into `DecodeError`.
    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let string_ty = Type::con("string", 0);
        let result_con = Type::con("Result", 2);
        let result_as = Type::app(Type::app(result_con, string_ty.clone()), a);
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(string_ty.clone(), result_as));

        #[derive(Clone)]
        struct Tags {
            null: Symbol,
            bool_: Symbol,
            string: Symbol,
            number: Symbol,
            array: Symbol,
            object: Symbol,
        }

        let tags = Tags {
            null: sym(&virtual_export_name("std.json", "Null")),
            bool_: sym(&virtual_export_name("std.json", "Bool")),
            string: sym(&virtual_export_name("std.json", "String")),
            number: sym(&virtual_export_name("std.json", "Number")),
            array: sym(&virtual_export_name("std.json", "Array")),
            object: sym(&virtual_export_name("std.json", "Object")),
        };

        fn to_json_value(
            v: &serde_json::Value,
            tags: &Tags,
            heap: &Heap,
        ) -> Result<Value, EngineError> {
            match v {
                serde_json::Value::Null => {
                    heap.alloc_adt(tags.null.clone(), vec![])?.get_value(heap)
                }
                serde_json::Value::Bool(b) => {
                    let value = heap.alloc_bool(*b)?.get_value(heap)?;
                    heap.alloc_adt(tags.bool_.clone(), vec![value])?
                        .get_value(heap)
                }
                serde_json::Value::String(s) => {
                    let value = heap.alloc_string(s.clone())?.get_value(heap)?;
                    heap.alloc_adt(tags.string.clone(), vec![value])?
                        .get_value(heap)
                }
                serde_json::Value::Number(n) => {
                    let Some(f) = n.as_f64() else {
                        return Err(EngineError::Custom(
                            "expected JSON number representable as f64".into(),
                        ));
                    };
                    let value = heap.alloc_f64(f)?.get_value(heap)?;
                    heap.alloc_adt(tags.number.clone(), vec![value])?
                        .get_value(heap)
                }
                serde_json::Value::Array(xs) => {
                    let mut out = Vec::with_capacity(xs.len());
                    for x in xs {
                        out.push(to_json_value(x, tags, heap)?);
                    }
                    let array = heap.alloc_array(out)?.get_value(heap)?;
                    heap.alloc_adt(tags.array.clone(), vec![array])?
                        .get_value(heap)
                }
                serde_json::Value::Object(obj) => {
                    let mut out = BTreeMap::new();
                    for (k, v) in obj {
                        out.insert(intern(k.as_str()), to_json_value(v, tags, heap)?);
                    }
                    let dict = heap.alloc_dict(out)?.get_value(heap)?;
                    heap.alloc_adt(tags.object.clone(), vec![dict])?
                        .get_value(heap)
                }
            }
        }

        fn result_ok(heap: &Heap, v: Value) -> Result<Value, EngineError> {
            heap.alloc_adt(sym("Ok"), vec![v])?.get_value(heap)
        }

        fn result_err(heap: &Heap, msg: String) -> Result<Value, EngineError> {
            let msg = heap.alloc_string(msg)?.get_value(heap)?;
            heap.alloc_adt(sym("Err"), vec![msg])?.get_value(heap)
        }

        engine.inject_native_scheme_typed(
            "prim_json_parse",
            scheme,
            1,
            move |engine, _call_type, args| {
                let Value::String(s) = &args[0] else {
                    return Err(EngineError::NativeType {
                        name: sym("prim_json_parse"),
                        expected: "string".into(),
                        got: args[0].type_name().into(),
                    });
                };
                let parsed: serde_json::Value = match serde_json::from_str(s) {
                    Ok(v) => v,
                    Err(e) => return result_err(engine.heap(), e.to_string()),
                };
                match to_json_value(&parsed, &tags, engine.heap()) {
                    Ok(v) => result_ok(engine.heap(), v),
                    Err(err) => result_err(engine.heap(), err.to_string()),
                }
            },
        )?;
    }

    Ok(())
}

pub(crate) fn inject_list_builtins(engine: &mut Engine) -> Result<(), EngineError> {
    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = Type::list(a.clone());
        let list_b = Type::list(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(list_a.clone(), list_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty, "prim_map")?;
            let values = list_to_vec(&args[1], "prim_map")?;
            let out = map_values(engine, args[0].clone(), &func_ty, &elem_ty, values)?;
            list_from_vec(engine.heap(), out)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = Type::array(a.clone());
        let array_b = Type::array(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty, "prim_map")?;
            let values = expect_array(&args[1], "prim_map")?;
            let out = map_values(engine, args[0].clone(), &func_ty, &elem_ty, values)?;
            engine.heap().alloc_array(out)?.get_value(engine.heap())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(a, array_a));
        engine.inject_native_scheme_typed(
            "prim_array_singleton",
            scheme,
            1,
            |engine, _call_type, args| {
                engine
                    .heap()
                    .alloc_array(vec![args[0].clone()])?
                    .get_value(engine.heap())
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = Type::option(a.clone());
        let opt_b = Type::option(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(opt_a.clone(), opt_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty, "prim_map")?;
            match option_value(&args[1])? {
                Some(v) => {
                    let mapped = apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty))?;
                    option_from_value(engine.heap(), Some(mapped))
                }
                None => option_from_value(engine.heap(), None),
            }
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let e_tv = engine.fresh_type_var(Some("e".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let e = Type::var(e_tv.clone());
        let result_a = Type::result(a.clone(), e.clone());
        let result_b = Type::result(b.clone(), e.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv, e_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(result_a.clone(), result_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            let (ok_ty, _err_ty) = result_types(&result_ty, "prim_map")?;
            match result_value(&args[1])? {
                Ok(v) => {
                    let mapped = apply(engine, args[0].clone(), v, Some(&func_ty), Some(&ok_ty))?;
                    result_from_value(engine.heap(), Ok(mapped))
                }
                Err(e) => result_from_value(engine.heap(), Err(e)),
            }
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(list_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldl", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldl", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldl"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty, "prim_foldl")?;
            let values = list_to_vec(&args[2], "prim_foldl")?;
            foldl_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                values,
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(array_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldl", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldl", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let array_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldl"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = array_elem_type(&array_ty, "prim_foldl")?;
            let values = expect_array(&args[2], "prim_foldl")?;
            foldl_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                values,
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(opt_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldl", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldl", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldl"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty, "prim_foldl")?;
            foldl_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                option_value(&args[2])?.into_iter(),
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::fun(b.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(list_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldr", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldr", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldr"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty, "prim_foldr")?;
            let values = list_to_vec(&args[2], "prim_foldr")?;
            foldr_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                values,
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::fun(b.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(array_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldr", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldr", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let array_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldr"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = array_elem_type(&array_ty, "prim_foldr")?;
            let values = expect_array(&args[2], "prim_foldr")?;
            foldr_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                values,
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::fun(b.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(opt_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldr", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldr", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldr"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty, "prim_foldr")?;
            foldr_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                option_value(&args[2])?.into_iter().collect(),
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(list_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_fold", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_fold", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_fold"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty, "prim_fold")?;
            let values = list_to_vec(&args[2], "prim_fold")?;
            foldl_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                values,
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(array_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_fold", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_fold", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let array_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_fold"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = array_elem_type(&array_ty, "prim_fold")?;
            let values = expect_array(&args[2], "prim_fold")?;
            foldl_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                values,
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(opt_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_fold", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_fold", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_fold"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty, "prim_fold")?;
            foldl_values(
                engine,
                args[0].clone(),
                &func_ty,
                &acc_ty,
                &elem_ty,
                args[1].clone(),
                option_value(&args[2])?.into_iter(),
            )
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::con("bool", 0)),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_filter",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_filter", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty, "prim_filter")?;
                let values = list_to_vec(&args[1], "prim_filter")?;
                let out = filter_values(
                    engine,
                    "prim_filter",
                    args[0].clone(),
                    &func_ty,
                    &elem_ty,
                    values,
                )?;
                list_from_vec(engine.heap(), out)
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::con("bool", 0)),
                Type::fun(array_a.clone(), array_a),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_filter",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_filter", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty, "prim_filter")?;
                let values = expect_array(&args[1], "prim_filter")?;
                let out = filter_values(
                    engine,
                    "prim_filter",
                    args[0].clone(),
                    &func_ty,
                    &elem_ty,
                    values,
                )?;
                engine.heap().alloc_array(out)?.get_value(engine.heap())
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::con("bool", 0)),
                Type::fun(opt_a.clone(), opt_a),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_filter",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_filter", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty, "prim_filter")?;
                match option_value(&args[1])? {
                    Some(v) => {
                        let keep =
                            apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty))?;
                        if expect_bool(&keep, "prim_filter")? {
                            Ok(args[1].clone())
                        } else {
                            option_from_value(engine.heap(), None)
                        }
                    }
                    None => option_from_value(engine.heap(), None),
                }
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = Type::list(a.clone());
        let list_b = Type::list(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::option(b.clone())),
                Type::fun(list_a.clone(), list_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_filter_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_filter_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty, "prim_filter_map")?;
                let values = list_to_vec(&args[1], "prim_filter_map")?;
                let out = filter_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values)?;
                list_from_vec(engine.heap(), out)
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = Type::array(a.clone());
        let array_b = Type::array(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::option(b.clone())),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_filter_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_filter_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty, "prim_filter_map")?;
                let values = expect_array(&args[1], "prim_filter_map")?;
                let out = filter_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values)?;
                engine.heap().alloc_array(out)?.get_value(engine.heap())
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = Type::option(a.clone());
        let opt_b = Type::option(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::option(b.clone())),
                Type::fun(opt_a.clone(), opt_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_filter_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_filter_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty, "prim_filter_map")?;
                match option_value(&args[1])? {
                    Some(v) => {
                        let mapped =
                            apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty))?;
                        Ok(mapped)
                    }
                    None => option_from_value(engine.heap(), None),
                }
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = Type::list(a.clone());
        let list_b = Type::list(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), list_b.clone()),
                Type::fun(list_a.clone(), list_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_flat_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty, "prim_flat_map")?;
                let values = list_to_vec(&args[1], "prim_flat_map")?;
                let out =
                    flat_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values, |v| {
                        list_to_vec(v, "prim_flat_map")
                    })?;
                list_from_vec(engine.heap(), out)
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = Type::array(a.clone());
        let array_b = Type::array(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), array_b.clone()),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_flat_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty, "prim_flat_map")?;
                let values = expect_array(&args[1], "prim_flat_map")?;
                let out =
                    flat_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values, |v| {
                        expect_array(v, "prim_flat_map")
                    })?;
                engine.heap().alloc_array(out)?.get_value(engine.heap())
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = Type::option(a.clone());
        let opt_b = Type::option(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), opt_b.clone()),
                Type::fun(opt_a.clone(), opt_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_flat_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty, "prim_flat_map")?;
                match option_value(&args[1])? {
                    Some(v) => apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty)),
                    None => option_from_value(engine.heap(), None),
                }
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let e_tv = engine.fresh_type_var(Some("e".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let e = Type::var(e_tv.clone());
        let result_a = Type::result(a.clone(), e.clone());
        let result_b = Type::result(b.clone(), e.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv, e_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), result_b.clone()),
                Type::fun(result_a.clone(), result_b),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_flat_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let result_ty = arg_tys[1].clone();
                let (ok_ty, _err_ty) = result_types(&result_ty, "prim_flat_map")?;
                match result_value(&args[1])? {
                    Ok(v) => {
                        let mapped =
                            apply(engine, args[0].clone(), v, Some(&func_ty), Some(&ok_ty))?;
                        let _ = result_value(&mapped)?;
                        Ok(mapped)
                    }
                    Err(e) => result_from_value(engine.heap(), Err(e)),
                }
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(list_a.clone(), list_a.clone()),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_or_else",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                if !list_to_vec(&args[1], "prim_or_else")?.is_empty() {
                    return Ok(args[1].clone());
                }
                apply(
                    engine,
                    args[0].clone(),
                    args[1].clone(),
                    Some(&func_ty),
                    Some(&list_ty),
                )
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(array_a.clone(), array_a.clone()),
                Type::fun(array_a.clone(), array_a),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_or_else",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                if !expect_array(&args[1], "prim_or_else")?.is_empty() {
                    return Ok(args[1].clone());
                }
                apply(
                    engine,
                    args[0].clone(),
                    args[1].clone(),
                    Some(&func_ty),
                    Some(&array_ty),
                )
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(opt_a.clone(), opt_a.clone()),
                Type::fun(opt_a.clone(), opt_a),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_or_else",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                if option_value(&args[1])?.is_some() {
                    return Ok(args[1].clone());
                }
                apply(
                    engine,
                    args[0].clone(),
                    args[1].clone(),
                    Some(&func_ty),
                    Some(&opt_ty),
                )
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let e_tv = engine.fresh_type_var(Some("e".into()));
        let a = Type::var(a_tv.clone());
        let e = Type::var(e_tv.clone());
        let result_a = Type::result(a.clone(), e.clone());
        let scheme = Scheme::new(
            vec![a_tv, e_tv],
            vec![],
            Type::fun(
                Type::fun(result_a.clone(), result_a.clone()),
                Type::fun(result_a.clone(), result_a),
            ),
        );
        engine.inject_native_scheme_typed(
            "prim_or_else",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let result_ty = arg_tys[1].clone();
                if result_value(&args[1])?.is_err() {
                    apply(
                        engine,
                        args[0].clone(),
                        args[1].clone(),
                        Some(&func_ty),
                        Some(&result_ty),
                    )
                } else {
                    Ok(args[1].clone())
                }
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("sum", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "sum")?;
            let mut values = list_to_vec(&args[0], "sum")?;
            if values.is_empty() {
                return engine.resolve_global_value(&sym("zero"), &elem_ty);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let acc = values.remove(0);
            foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc, values)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("sum", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "sum")?;
            let mut values = expect_array(&args[0], "sum")?;
            if values.is_empty() {
                return engine.resolve_global_value(&sym("zero"), &elem_ty);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let acc = values.remove(0);
            foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc, values)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("sum", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty, "sum")?;
            match option_value(&args[0])? {
                Some(v) => Ok(v),
                None => engine.resolve_global_value(&sym("zero"), &elem_ty),
            }
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("mean", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "mean")?;
            let mut values = list_to_vec(&args[0], "mean")?;
            let len = values.len();
            if len == 0 {
                return Err(EngineError::EmptySequence);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let div = resolve_binary_op(engine, "/", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
            let acc0 = values.remove(0);
            let acc = foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc0, values)?;
            let len_val = len_value_for_type(engine.heap(), &elem_ty, len, "mean")?;
            let div_step = apply(engine, div.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
            apply(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty))
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("mean", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "mean")?;
            let mut values = expect_array(&args[0], "mean")?;
            let len = values.len();
            if len == 0 {
                return Err(EngineError::EmptySequence);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let div = resolve_binary_op(engine, "/", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
            let acc0 = values.remove(0);
            let acc = foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc0, values)?;
            let len_val = len_value_for_type(engine.heap(), &elem_ty, len, "mean")?;
            let div_step = apply(engine, div.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
            apply(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty))
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("mean", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty, "mean")?;
            match option_value(&args[0])? {
                Some(v) => {
                    let len_val = len_value_for_type(engine.heap(), &elem_ty, 1, "mean")?;
                    let div = resolve_binary_op(engine, "/", &elem_ty)?;
                    let div_ty =
                        Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
                    let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
                    let div_step = apply(engine, div.clone(), v, Some(&div_ty), Some(&elem_ty))?;
                    apply(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty))
                }
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), Type::con("i32", 0)),
        );
        engine.inject_native_scheme_typed("count", scheme, 1, |engine, _call_type, args| {
            engine
                .heap()
                .alloc_i32(list_to_vec(&args[0], "count")?.len() as i32)?
                .get_value(engine.heap())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), Type::con("i32", 0)),
        );
        engine.inject_native_scheme_typed("count", scheme, 1, |engine, _call_type, args| {
            engine
                .heap()
                .alloc_i32(expect_array(&args[0], "count")?.len() as i32)?
                .get_value(engine.heap())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), Type::con("i32", 0)),
        );
        engine.inject_native_scheme_typed("count", scheme, 1, |engine, _call_type, args| {
            engine
                .heap()
                .alloc_i32(option_value(&args[0])?.is_some() as i32)?
                .get_value(engine.heap())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(list_a.clone(), list_a)),
        );
        engine.inject_native_scheme_typed("prim_take", scheme, 2, |engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_take")?;
            let n = as_nonneg_usize(n);
            let xs = list_to_vec(&args[1], "prim_take")?;
            list_from_vec(engine.heap(), xs.into_iter().take(n).collect())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(array_a.clone(), array_a)),
        );
        engine.inject_native_scheme_typed("prim_take", scheme, 2, |engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_take")?;
            let n = as_nonneg_usize(n);
            let xs = expect_array(&args[1], "prim_take")?;
            engine
                .heap()
                .alloc_array(xs.into_iter().take(n).collect())?
                .get_value(engine.heap())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(list_a.clone(), list_a)),
        );
        engine.inject_native_scheme_typed("prim_skip", scheme, 2, |engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_skip")?;
            let n = as_nonneg_usize(n);
            let xs = list_to_vec(&args[1], "prim_skip")?;
            list_from_vec(engine.heap(), xs.into_iter().skip(n).collect())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(array_a.clone(), array_a)),
        );
        engine.inject_native_scheme_typed("prim_skip", scheme, 2, |engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_skip")?;
            let n = as_nonneg_usize(n);
            let xs = expect_array(&args[1], "prim_skip")?;
            engine
                .heap()
                .alloc_array(xs.into_iter().skip(n).collect())?
                .get_value(engine.heap())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(list_a.clone(), a.clone())),
        );
        engine.inject_native_scheme_typed("prim_get", scheme, 2, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_get", call_type, 2)?;
            let list_ty = arg_tys[1].clone();
            let _elem_ty = list_elem_type(&list_ty, "prim_get")?;
            let idx = i32::from_value(&args[0], "prim_get")?;
            let xs = list_to_vec(&args[1], "prim_get")?;
            let idx = checked_index(sym("prim_get"), idx, xs.len())?;
            Ok(xs[idx].clone())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(array_a.clone(), a.clone())),
        );
        engine.inject_native_scheme_typed("prim_get", scheme, 2, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_get", call_type, 2)?;
            let array_ty = arg_tys[1].clone();
            let _elem_ty = array_elem_type(&array_ty, "prim_get")?;
            let idx = i32::from_value(&args[0], "prim_get")?;
            let xs = expect_array(&args[1], "prim_get")?;
            let idx = checked_index(sym("prim_get"), idx, xs.len())?;
            Ok(xs[idx].clone())
        })?;
    }

    for size in 2..=32 {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let tuple = Type::tuple(vec![a.clone(); size]);
        let scheme = Scheme::new(
            vec![a_tv],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(tuple.clone(), a.clone())),
        );
        engine.inject_native_scheme_typed(
            "prim_get",
            scheme,
            2,
            move |_engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain("prim_get", call_type, 2)?;
                let tuple_ty = arg_tys[1].clone();
                let _elem_ty = tuple_elem_type(&tuple_ty, "prim_get")?;
                let idx = i32::from_value(&args[0], "prim_get")?;
                let idx_usize = checked_index(sym("prim_get"), idx, size)?;
                match &args[1] {
                    Value::Tuple(xs) => {
                        if xs.len() != size {
                            return Err(EngineError::NativeType {
                                name: sym("prim_get"),
                                expected: format!("tuple{}", size),
                                got: format!("tuple{}", xs.len()),
                            });
                        }
                        Ok(xs[idx_usize].clone())
                    }
                    other => Err(EngineError::NativeType {
                        name: sym("prim_get"),
                        expected: format!("tuple{}", size),
                        got: other.type_name().into(),
                    }),
                }
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = Type::list(a.clone());
        let list_b = Type::list(b.clone());
        let list_pair = Type::list(Type::tuple(vec![a.clone(), b.clone()]));
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(list_a.clone(), Type::fun(list_b.clone(), list_pair)),
        );
        engine.inject_native_scheme_typed("prim_zip", scheme, 2, |engine, _call_type, args| {
            let xs = list_to_vec(&args[0], "prim_zip")?;
            let ys = list_to_vec(&args[1], "prim_zip")?;
            let zipped = zip_tuple2(engine.heap(), xs, ys)?;
            list_from_vec(engine.heap(), zipped)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = Type::array(a.clone());
        let array_b = Type::array(b.clone());
        let array_pair = Type::array(Type::tuple(vec![a.clone(), b.clone()]));
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_b.clone(), array_pair)),
        );
        engine.inject_native_scheme_typed("prim_zip", scheme, 2, |engine, _call_type, args| {
            let xs = expect_array(&args[0], "prim_zip")?;
            let ys = expect_array(&args[1], "prim_zip")?;
            let zipped = zip_tuple2(engine.heap(), xs, ys)?;
            engine.heap().alloc_array(zipped)?.get_value(engine.heap())
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_pair = Type::list(Type::tuple(vec![a.clone(), b.clone()]));
        let list_a = Type::list(a.clone());
        let list_b = Type::list(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(list_pair.clone(), Type::tuple(vec![list_a, list_b])),
        );
        engine.inject_native_scheme_typed(
            "prim_unzip",
            scheme,
            1,
            |engine, _call_type, args| {
                let pairs = list_to_vec(&args[0], "prim_unzip")?;
                let (left, right) = unzip_tuple2(sym("prim_unzip"), pairs)?;
                let left = list_from_vec(engine.heap(), left)?;
                let right = list_from_vec(engine.heap(), right)?;
                engine
                    .heap()
                    .alloc_tuple(vec![left, right])?
                    .get_value(engine.heap())
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let b_tv = engine.fresh_type_var(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_pair = Type::array(Type::tuple(vec![a.clone(), b.clone()]));
        let array_a = Type::array(a.clone());
        let array_b = Type::array(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(array_pair.clone(), Type::tuple(vec![array_a, array_b])),
        );
        engine.inject_native_scheme_typed(
            "prim_unzip",
            scheme,
            1,
            |engine, _call_type, args| {
                let pairs = expect_array(&args[0], "prim_unzip")?;
                let (left, right) = unzip_tuple2(sym("prim_unzip"), pairs)?;
                let left = engine.heap().alloc_array(left)?.get_value(engine.heap())?;
                let right = engine.heap().alloc_array(right)?.get_value(engine.heap())?;
                engine
                    .heap()
                    .alloc_tuple(vec![left, right])?
                    .get_value(engine.heap())
            },
        )?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("min", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("min", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "min")?;
            let values = list_to_vec(&args[0], "min")?;
            extremum_by_type("min", &elem_ty, values, std::cmp::Ordering::Less)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("min", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("min", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "min")?;
            let values = expect_array(&args[0], "min")?;
            extremum_by_type("min", &elem_ty, values, std::cmp::Ordering::Less)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("min", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("min", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty, "min")?;
            match option_value(&args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("max", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("max", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "max")?;
            let values = list_to_vec(&args[0], "max")?;
            extremum_by_type("max", &elem_ty, values, std::cmp::Ordering::Greater)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("max", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("max", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "max")?;
            let values = expect_array(&args[0], "max")?;
            extremum_by_type("max", &elem_ty, values, std::cmp::Ordering::Greater)
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("max", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("max", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty, "max")?;
            match option_value(&args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    Ok(())
}

pub(crate) fn inject_option_result_builtins(engine: &mut Engine) -> Result<(), EngineError> {
    let is_some = sym("is_some");
    let is_some_scheme = engine.lookup_scheme(&is_some)?;
    engine.inject_native_scheme_typed(
        "is_some",
        is_some_scheme,
        1,
        |engine, _call_type, args| {
            engine
                .heap()
                .alloc_bool(option_value(&args[0])?.is_some())?
                .get_value(engine.heap())
        },
    )?;
    let is_none = sym("is_none");
    let is_none_scheme = engine.lookup_scheme(&is_none)?;
    engine.inject_native_scheme_typed(
        "is_none",
        is_none_scheme,
        1,
        |engine, _call_type, args| {
            engine
                .heap()
                .alloc_bool(option_value(&args[0])?.is_none())?
                .get_value(engine.heap())
        },
    )?;

    let is_ok = sym("is_ok");
    let is_ok_scheme = engine.lookup_scheme(&is_ok)?;
    engine.inject_native_scheme_typed("is_ok", is_ok_scheme, 1, |engine, _call_type, args| {
        engine
            .heap()
            .alloc_bool(result_value(&args[0])?.is_ok())?
            .get_value(engine.heap())
    })?;
    let is_err = sym("is_err");
    let is_err_scheme = engine.lookup_scheme(&is_err)?;
    engine.inject_native_scheme_typed("is_err", is_err_scheme, 1, |engine, _call_type, args| {
        engine
            .heap()
            .alloc_bool(result_value(&args[0])?.is_err())?
            .get_value(engine.heap())
    })?;
    Ok(())
}
