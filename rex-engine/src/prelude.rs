//! Prelude injection helpers for Rex.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use futures::FutureExt;
use rex_ast::expr::{Symbol, intern, sym, sym_eq};
use rex_ts::{Scheme, Type, TypeKind, Types, unify};
use uuid::Uuid;

use crate::Engine;
use crate::engine::{apply as apply_pointer, binary_arg_types};
use crate::value::{Heap, Pointer, list_to_vec};
use crate::virtual_export_name;
use crate::{EngineError, FromPointer, IntoPointer, OverloadedFn, Value};
use rex_util::GasMeter;

fn values_to_ptrs<T: IntoPointer>(
    heap: &Heap,
    values: Vec<T>,
) -> Result<Vec<Pointer>, EngineError> {
    values
        .into_iter()
        .map(|value| value.into_pointer(heap))
        .collect()
}

async fn invoke_pointer_fn<State: Clone + Sync + 'static>(
    engine: &Engine<State>,
    func: Pointer,
    arg: Pointer,
    func_ty: Option<&Type>,
    arg_ty: Option<&Type>,
) -> Result<Pointer, EngineError> {
    let mut gas = GasMeter::default();
    apply_pointer(engine, func, arg, func_ty, arg_ty, &mut gas).await
}

fn expect_list(heap: &Heap, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
    let value = heap.get(pointer)?;
    list_to_vec(heap, value.as_ref())
}

fn list_from_pointers(heap: &Heap, values: Vec<Pointer>) -> Result<Pointer, EngineError> {
    let mut list = heap.alloc_adt(sym("Empty"), vec![])?;
    for value in values.into_iter().rev() {
        list = heap.alloc_adt(sym("Cons"), vec![value, list])?;
    }
    Ok(list)
}

pub(crate) fn list_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "List")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            expected: "List a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn array_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Array")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            expected: "Array a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn dict_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Dict")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            expected: "Dict a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn option_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Option")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            expected: "Option a".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) fn result_types(typ: &Type) -> Result<(Type, Type), EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, ok) => match head.as_ref() {
            TypeKind::App(head, err) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Result")) => {
                Ok((ok.clone(), err.clone()))
            }
            _ => Err(EngineError::NativeType {
                expected: "Result a e".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            expected: "Result a e".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) async fn resolve_binary_op<State: Clone + Sync + 'static>(
    engine: &Engine<State>,
    name: &str,
    elem_ty: &Type,
) -> Result<Pointer, EngineError> {
    let op_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
    engine.resolve_global(&sym(name), &op_ty).await
}

pub(crate) fn len_value_for_type(
    heap: &Heap,
    elem_ty: &Type,
    len: usize,
) -> Result<Pointer, EngineError> {
    match elem_ty.as_ref() {
        TypeKind::Con(c) if sym_eq(&c.name, "f32") => heap.alloc_f32(len as f32),
        TypeKind::Con(c) if sym_eq(&c.name, "f64") => heap.alloc_f64(len as f64),
        _ => Err(EngineError::NativeType {
            expected: "f32 or f64".into(),
            got: elem_ty.to_string(),
        }),
    }
}

pub(crate) fn expect_array(heap: &Heap, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
    heap.pointer_as_array(pointer)
}

pub(crate) fn option_from_pointer(
    heap: &Heap,
    value: Option<Pointer>,
) -> Result<Pointer, EngineError> {
    match value {
        Some(v) => heap.alloc_adt(sym("Some"), vec![v]),
        None => heap.alloc_adt(sym("None"), vec![]),
    }
}

pub(crate) fn option_value(heap: &Heap, pointer: &Pointer) -> Result<Option<Pointer>, EngineError> {
    let (tag, args) = heap.pointer_as_adt(pointer)?;
    if sym_eq(&tag, "Some") && args.len() == 1 {
        Ok(Some(args[0].clone()))
    } else if sym_eq(&tag, "None") && args.is_empty() {
        Ok(None)
    } else {
        Err(EngineError::NativeType {
            expected: "Option".into(),
            got: heap.type_name(pointer)?.into(),
        })
    }
}

pub(crate) fn result_value(
    heap: &Heap,
    pointer: &Pointer,
) -> Result<Result<Pointer, Pointer>, EngineError> {
    let (tag, args) = heap.pointer_as_adt(pointer)?;
    if sym_eq(&tag, "Ok") && args.len() == 1 {
        Ok(Ok(args[0].clone()))
    } else if sym_eq(&tag, "Err") && args.len() == 1 {
        Ok(Err(args[0].clone()))
    } else {
        Err(EngineError::NativeType {
            expected: "Result".into(),
            got: heap.type_name(pointer)?.into(),
        })
    }
}

pub(crate) fn result_from_pointer(
    heap: &Heap,
    value: Result<Pointer, Pointer>,
) -> Result<Pointer, EngineError> {
    match value {
        Ok(v) => heap.alloc_adt(sym("Ok"), vec![v]),
        Err(v) => heap.alloc_adt(sym("Err"), vec![v]),
    }
}

pub(crate) fn split_fun_chain(typ: &Type, count: usize) -> Result<(Vec<Type>, Type), EngineError> {
    let mut args = Vec::with_capacity(count);
    let mut cur = typ.clone();
    for _ in 0..count {
        let (arg, rest) = match cur.as_ref() {
            TypeKind::Fun(arg, rest) => (arg.clone(), rest.clone()),
            _ => {
                return Err(EngineError::NativeType {
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

pub(crate) fn tuple_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::Tuple(elems) if !elems.is_empty() => {
            let first = elems[0].clone();
            for elem in elems.iter().skip(1) {
                if *elem != first {
                    return Err(EngineError::NativeType {
                        expected: first.to_string(),
                        got: elem.to_string(),
                    });
                }
            }
            Ok(first)
        }
        _ => Err(EngineError::NativeType {
            expected: "tuple".into(),
            got: typ.to_string(),
        }),
    }
}

pub(crate) async fn map_values<State: Clone + Sync + 'static, F, I, T>(
    engine: &Engine<State>,
    func: F,
    func_ty: &Type,
    elem_ty: &Type,
    values: I,
) -> Result<Vec<Pointer>, EngineError>
where
    F: IntoPointer,
    I: IntoIterator<Item = T>,
    T: IntoPointer,
{
    let func = func.into_pointer(engine.heap())?;
    let mut out = Vec::new();
    for value in values {
        let value = value.into_pointer(engine.heap())?;
        out.push(
            invoke_pointer_fn(engine, func.clone(), value, Some(func_ty), Some(elem_ty)).await?,
        );
    }
    Ok(out)
}

pub(crate) async fn filter_values<State: Clone + Sync + 'static, P, I, T>(
    engine: &Engine<State>,
    pred: P,
    pred_ty: &Type,
    elem_ty: &Type,
    values: I,
) -> Result<Vec<Pointer>, EngineError>
where
    P: IntoPointer,
    I: IntoIterator<Item = T>,
    T: IntoPointer,
{
    let pred = pred.into_pointer(engine.heap())?;
    let mut out = Vec::new();
    for value in values {
        let value = value.into_pointer(engine.heap())?;
        let keep = invoke_pointer_fn(
            engine,
            pred.clone(),
            value.clone(),
            Some(pred_ty),
            Some(elem_ty),
        )
        .await?;
        if bool::from_pointer(engine.heap(), &keep)? {
            out.push(value);
        }
    }
    Ok(out)
}

pub(crate) async fn filter_map_values<State: Clone + Sync + 'static, F, I, T>(
    engine: &Engine<State>,
    func: F,
    func_ty: &Type,
    elem_ty: &Type,
    values: I,
) -> Result<Vec<Pointer>, EngineError>
where
    F: IntoPointer,
    I: IntoIterator<Item = T>,
    T: IntoPointer,
{
    let func = func.into_pointer(engine.heap())?;
    let mut out = Vec::new();
    for value in values {
        let value = value.into_pointer(engine.heap())?;
        let mapped =
            invoke_pointer_fn(engine, func.clone(), value, Some(func_ty), Some(elem_ty)).await?;
        if let Some(v) = option_value(engine.heap(), &mapped)? {
            out.push(v);
        }
    }
    Ok(out)
}

pub(crate) async fn flat_map_values<State: Clone + Sync + 'static, F, I, T>(
    engine: &Engine<State>,
    func: F,
    func_ty: &Type,
    elem_ty: &Type,
    values: I,
    mut extract: impl FnMut(&Pointer) -> Result<Vec<Pointer>, EngineError>,
) -> Result<Vec<Pointer>, EngineError>
where
    F: IntoPointer,
    I: IntoIterator<Item = T>,
    T: IntoPointer,
{
    let func = func.into_pointer(engine.heap())?;
    let mut out = Vec::new();
    for value in values {
        let value = value.into_pointer(engine.heap())?;
        let mapped =
            invoke_pointer_fn(engine, func.clone(), value, Some(func_ty), Some(elem_ty)).await?;
        out.extend(extract(&mapped)?);
    }
    Ok(out)
}

pub(crate) async fn foldl_values<State: Clone + Sync + 'static>(
    engine: &Engine<State>,
    func: Pointer,
    func_ty: &Type,
    acc_ty: &Type,
    elem_ty: &Type,
    mut acc: Pointer,
    values: impl IntoIterator<Item = Pointer>,
) -> Result<Pointer, EngineError> {
    let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
    for value in values {
        let step =
            invoke_pointer_fn(engine, func.clone(), acc, Some(func_ty), Some(acc_ty)).await?;
        acc = invoke_pointer_fn(engine, step, value, Some(&step_ty), Some(elem_ty)).await?;
    }
    Ok(acc)
}

pub(crate) async fn foldr_values<State: Clone + Sync + 'static>(
    engine: &Engine<State>,
    func: Pointer,
    func_ty: &Type,
    acc_ty: &Type,
    elem_ty: &Type,
    mut acc: Pointer,
    values: Vec<Pointer>,
) -> Result<Pointer, EngineError> {
    let step_ty = Type::fun(acc_ty.clone(), acc_ty.clone());
    for value in values.into_iter().rev() {
        let step =
            invoke_pointer_fn(engine, func.clone(), value, Some(func_ty), Some(elem_ty)).await?;
        acc = invoke_pointer_fn(engine, step, acc, Some(&step_ty), Some(acc_ty)).await?;
    }
    Ok(acc)
}

pub(crate) fn extremum_by_type(
    heap: &Heap,
    name: &'static str,
    elem_ty: &Type,
    values: Vec<Pointer>,
    choose: std::cmp::Ordering,
) -> Result<Pointer, EngineError> {
    let name = sym(name);
    let mut values = values.into_iter();
    let mut best = values.next().ok_or(EngineError::EmptySequence)?;
    for value in values {
        let value_ref = heap.get(&value)?;
        let best_ref = heap.get(&best)?;
        let ord = cmp_value_by_type(heap, &name, elem_ty, value_ref.as_ref(), best_ref.as_ref())?;
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
    xs: Vec<Pointer>,
    ys: Vec<Pointer>,
) -> Result<Vec<Pointer>, EngineError> {
    xs.into_iter()
        .zip(ys)
        .map(|(x, y)| heap.alloc_tuple(vec![x, y]))
        .collect()
}

pub(crate) fn unzip_tuple2(
    heap: &Heap,
    pairs: Vec<Pointer>,
) -> Result<(Vec<Pointer>, Vec<Pointer>), EngineError> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for pair in pairs {
        let elems = heap.pointer_as_tuple(&pair)?;
        let len = elems.len();
        if len != 2 {
            return Err(EngineError::NativeType {
                expected: "tuple2".into(),
                got: format!("tuple{len}"),
            });
        }
        left.push(elems[0].clone());
        right.push(elems[1].clone());
    }
    Ok((left, right))
}

pub(crate) fn as_nonneg_usize(n: i32) -> usize {
    if n <= 0 { 0 } else { n as usize }
}

fn cmp_value_by_type(
    heap: &Heap,
    op_name: &Symbol,
    typ: &Type,
    lhs: &Value,
    rhs: &Value,
) -> Result<std::cmp::Ordering, EngineError> {
    fn mismatch(
        heap: &Heap,
        op_name: &Symbol,
        expected: &str,
        lhs: &Value,
        rhs: &Value,
    ) -> EngineError {
        let _ = op_name;
        EngineError::NativeType {
            expected: expected.to_string(),
            got: format!(
                "{}, {}",
                heap.type_name_of_value(lhs),
                heap.type_name_of_value(rhs)
            ),
        }
    }

    match typ.as_ref() {
        TypeKind::Con(tc) => match tc.name.as_ref() {
            "u8" => {
                let a = lhs
                    .value_as_u8()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_u8()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "u16" => {
                let a = lhs
                    .value_as_u16()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_u16()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "u32" => {
                let a = lhs
                    .value_as_u32()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_u32()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "u64" => {
                let a = lhs
                    .value_as_u64()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_u64()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "i8" => {
                let a = lhs
                    .value_as_i8()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_i8()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "i16" => {
                let a = lhs
                    .value_as_i16()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_i16()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "i32" => {
                let a = lhs
                    .value_as_i32()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_i32()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "i64" => {
                let a = lhs
                    .value_as_i64()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_i64()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "f32" => {
                let a = lhs
                    .value_as_f32()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_f32()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                    expected: tc.name.to_string(),
                    got: "nan".into(),
                })
            }
            "f64" => {
                let a = lhs
                    .value_as_f64()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_f64()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                    expected: tc.name.to_string(),
                    got: "nan".into(),
                })
            }
            "string" => {
                let a = lhs
                    .value_as_string()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_string()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "uuid" => {
                let a = lhs
                    .value_as_uuid()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_uuid()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            "datetime" => {
                let a = lhs
                    .value_as_datetime()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                let b = rhs
                    .value_as_datetime()
                    .map_err(|_| mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            _ => Err(mismatch(heap, op_name, tc.name.as_ref(), lhs, rhs)),
        },
        _ => Err(mismatch(heap, op_name, &typ.to_string(), lhs, rhs)),
    }
}

pub(crate) fn inject_prelude_adts<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
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

pub(crate) fn inject_equality_ops<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    // Equality primitives are monomorphic overloads (same name, different
    // concrete types), matching the numeric `prim_add` style.
    engine.export("prim_eq", |_: &State, a: bool, b: bool| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: bool, b: bool| -> bool { a != b })?;

    engine.export("prim_eq", |_: &State, a: u8, b: u8| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: u8, b: u8| -> bool { a != b })?;
    engine.export("prim_eq", |_: &State, a: u16, b: u16| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: u16, b: u16| -> bool { a != b })?;
    engine.export("prim_eq", |_: &State, a: u32, b: u32| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: u32, b: u32| -> bool { a != b })?;
    engine.export("prim_eq", |_: &State, a: u64, b: u64| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: u64, b: u64| -> bool { a != b })?;

    engine.export("prim_eq", |_: &State, a: i8, b: i8| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: i8, b: i8| -> bool { a != b })?;
    engine.export("prim_eq", |_: &State, a: i16, b: i16| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: i16, b: i16| -> bool { a != b })?;
    engine.export("prim_eq", |_: &State, a: i32, b: i32| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: i32, b: i32| -> bool { a != b })?;
    engine.export("prim_eq", |_: &State, a: i64, b: i64| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: i64, b: i64| -> bool { a != b })?;

    engine.export("prim_eq", |_: &State, a: f32, b: f32| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: f32, b: f32| -> bool { a != b })?;
    engine.export("prim_eq", |_: &State, a: f64, b: f64| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: f64, b: f64| -> bool { a != b })?;

    engine.export("prim_eq", |_: &State, a: String, b: String| -> bool {
        a == b
    })?;
    engine.export("prim_ne", |_: &State, a: String, b: String| -> bool {
        a != b
    })?;
    engine.export("prim_eq", |_: &State, a: Uuid, b: Uuid| -> bool { a == b })?;
    engine.export("prim_ne", |_: &State, a: Uuid, b: Uuid| -> bool { a != b })?;
    engine.export(
        "prim_eq",
        |_: &State, a: DateTime<Utc>, b: DateTime<Utc>| -> bool { a == b },
    )?;
    engine.export(
        "prim_ne",
        |_: &State, a: DateTime<Utc>, b: DateTime<Utc>| -> bool { a != b },
    )?;

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
        engine.export_native_async(
            "prim_array_eq",
            scheme.clone(),
            2,
            |engine, call_type, args| {
                async move {
                    let (lhs_ty, rhs_ty) = binary_arg_types(&call_type)?;
                    let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
                        expected: lhs_ty.to_string(),
                        got: rhs_ty.to_string(),
                    })?;
                    let array_ty = lhs_ty.apply(&subst);
                    let elem_ty = array_elem_type(&array_ty)?;
                    let xs = expect_array(engine.heap(), &args[0])?;
                    let ys = expect_array(engine.heap(), &args[1])?;
                    if xs.len() != ys.len() {
                        return engine.heap().alloc_bool(false);
                    }

                    let bool_ty = Type::con("bool", 0);
                    let eq_ty =
                        Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), bool_ty.clone()));
                    let step_ty = Type::fun(elem_ty.clone(), bool_ty);
                    for (x, y) in xs.iter().zip(ys.iter()) {
                        let (name, typ, applied, applied_types) =
                            OverloadedFn::new(sym("=="), eq_ty.clone()).into_parts();
                        let f =
                            engine
                                .heap()
                                .alloc_overloaded(name, typ, applied, applied_types)?;
                        let x = x.clone();
                        let f =
                            invoke_pointer_fn(engine, f, x, Some(&eq_ty), Some(&elem_ty)).await?;
                        let y = y.clone();
                        let r =
                            invoke_pointer_fn(engine, f, y, Some(&step_ty), Some(&elem_ty)).await?;
                        if !bool::from_pointer(engine.heap(), &r)? {
                            return engine.heap().alloc_bool(false);
                        }
                    }
                    engine.heap().alloc_bool(true)
                }
                .boxed_local()
            },
        )?;

        let scheme = Scheme::new(
            vec![a_tv],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_a, bool_ty.clone())),
        );
        engine.export_native_async("prim_array_ne", scheme, 2, |engine, call_type, args| {
            async move {
                let eq = engine
                    .call_native_impl("prim_array_eq", &call_type, &args)
                    .await?;
                engine
                    .heap()
                    .alloc_bool(!bool::from_pointer(engine.heap(), &eq)?)
            }
            .boxed_local()
        })?;
    }

    Ok(())
}

pub(crate) fn inject_order_ops<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    fn cmp_to_i32(ord: std::cmp::Ordering) -> i32 {
        match ord {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    // Integer and string comparisons can be injected as direct typed natives,
    // with no runtime type switching.
    engine.export("prim_lt", |_: &State, a: u8, b: u8| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: u8, b: u8| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: u8, b: u8| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: u8, b: u8| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: u8, b: u8| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: u16, b: u16| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: u16, b: u16| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: u16, b: u16| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: u16, b: u16| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: u16, b: u16| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: u32, b: u32| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: u32, b: u32| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: u32, b: u32| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: u32, b: u32| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: u32, b: u32| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: u64, b: u64| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: u64, b: u64| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: u64, b: u64| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: u64, b: u64| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: u64, b: u64| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: i8, b: i8| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: i8, b: i8| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: i8, b: i8| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: i8, b: i8| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: i8, b: i8| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: i16, b: i16| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: i16, b: i16| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: i16, b: i16| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: i16, b: i16| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: i16, b: i16| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: i32, b: i32| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: i32, b: i32| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: i32, b: i32| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: i32, b: i32| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: i32, b: i32| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: i64, b: i64| -> bool { a < b })?;
    engine.export("prim_le", |_: &State, a: i64, b: i64| -> bool { a <= b })?;
    engine.export("prim_gt", |_: &State, a: i64, b: i64| -> bool { a > b })?;
    engine.export("prim_ge", |_: &State, a: i64, b: i64| -> bool { a >= b })?;
    engine.export("prim_cmp", |_: &State, a: i64, b: i64| -> i32 {
        cmp_to_i32(a.cmp(&b))
    })?;

    engine.export("prim_lt", |_: &State, a: String, b: String| -> bool {
        a < b
    })?;
    engine.export("prim_le", |_: &State, a: String, b: String| -> bool {
        a <= b
    })?;
    engine.export("prim_gt", |_: &State, a: String, b: String| -> bool {
        a > b
    })?;
    engine.export("prim_ge", |_: &State, a: String, b: String| -> bool {
        a >= b
    })?;
    engine.export("prim_cmp", |_: &State, a: String, b: String| -> i32 {
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
        engine.export_native(name, scheme, 2, move |engine, _, args| {
            let a = f32::from_pointer(engine.heap(), &args[0])?;
            let b = f32::from_pointer(engine.heap(), &args[1])?;
            let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                expected: "f32".into(),
                got: "nan".into(),
            })?;
            engine.heap().alloc_bool(pred(ord))
        })?;
    }
    engine.export_native("prim_cmp", f32_cmp, 2, |engine, _, args| {
        let a = f32::from_pointer(engine.heap(), &args[0])?;
        let b = f32::from_pointer(engine.heap(), &args[1])?;
        let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
            expected: "f32".into(),
            got: "nan".into(),
        })?;
        engine.heap().alloc_i32(cmp_to_i32(ord))
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
        engine.export_native(name, scheme, 2, move |engine, _, args| {
            let a = f64::from_pointer(engine.heap(), &args[0])?;
            let b = f64::from_pointer(engine.heap(), &args[1])?;
            let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                expected: "f64".into(),
                got: "nan".into(),
            })?;
            engine.heap().alloc_bool(pred(ord))
        })?;
    }
    engine.export_native("prim_cmp", f64_cmp, 2, |engine, _, args| {
        let a = f64::from_pointer(engine.heap(), &args[0])?;
        let b = f64::from_pointer(engine.heap(), &args[1])?;
        let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
            expected: "f64".into(),
            got: "nan".into(),
        })?;
        engine.heap().alloc_i32(cmp_to_i32(ord))
    })?;

    Ok(())
}

pub(crate) fn inject_pretty_ops<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    engine.export("prim_pretty", |_: &State, x: bool| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: u8| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: u16| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: u32| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: u64| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: i8| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: i16| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: i32| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: i64| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: f32| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: f64| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: String| -> String { x })?;
    engine.export("prim_pretty", |_: &State, x: Uuid| -> String {
        x.to_string()
    })?;
    engine.export("prim_pretty", |_: &State, x: DateTime<Utc>| -> String {
        x.to_string()
    })?;
    Ok(())
}

pub(crate) fn inject_boolean_ops<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    engine.export("(&&)", |_: &State, a: bool, b: bool| -> bool { a && b })?;
    engine.export("(||)", |_: &State, a: bool, b: bool| -> bool { a || b })?;
    Ok(())
}

pub(crate) fn inject_numeric_ops<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    // Additive identity
    engine.export_value("prim_zero", String::new())?;
    engine.export_value("prim_zero", 0u8)?;
    engine.export_value("prim_zero", 0u16)?;
    engine.export_value("prim_zero", 0u32)?;
    engine.export_value("prim_zero", 0u64)?;
    engine.export_value("prim_zero", 0i8)?;
    engine.export_value("prim_zero", 0i16)?;
    engine.export_value("prim_zero", 0i32)?;
    engine.export_value("prim_zero", 0i64)?;
    engine.export_value("prim_zero", 0.0f32)?;
    engine.export_value("prim_zero", 0.0f64)?;

    // Multiplicative identity
    engine.export_value("prim_one", 1u8)?;
    engine.export_value("prim_one", 1u16)?;
    engine.export_value("prim_one", 1u32)?;
    engine.export_value("prim_one", 1u64)?;
    engine.export_value("prim_one", 1i8)?;
    engine.export_value("prim_one", 1i16)?;
    engine.export_value("prim_one", 1i32)?;
    engine.export_value("prim_one", 1i64)?;
    engine.export_value("prim_one", 1.0f32)?;
    engine.export_value("prim_one", 1.0f64)?;

    // Addition
    engine.export("prim_add", |_: &State, a: u8, b: u8| -> u8 { a + b })?;
    engine.export("prim_add", |_: &State, a: u16, b: u16| -> u16 { a + b })?;
    engine.export("prim_add", |_: &State, a: u32, b: u32| -> u32 { a + b })?;
    engine.export("prim_add", |_: &State, a: u64, b: u64| -> u64 { a + b })?;
    engine.export("prim_add", |_: &State, a: i8, b: i8| -> i8 { a + b })?;
    engine.export("prim_add", |_: &State, a: i16, b: i16| -> i16 { a + b })?;
    engine.export("prim_add", |_: &State, a: i32, b: i32| -> i32 { a + b })?;
    engine.export("prim_add", |_: &State, a: i64, b: i64| -> i64 { a + b })?;
    engine.export("prim_add", |_: &State, a: f32, b: f32| -> f32 { a + b })?;
    engine.export("prim_add", |_: &State, a: f64, b: f64| -> f64 { a + b })?;
    engine.export("prim_add", |_: &State, a: String, b: String| -> String {
        format!("{}{}", a, b)
    })?;

    // Subtraction and negation
    engine.export("prim_sub", |_: &State, a: i8, b: i8| -> i8 { a - b })?;
    engine.export("prim_sub", |_: &State, a: i16, b: i16| -> i16 { a - b })?;
    engine.export("prim_sub", |_: &State, a: i32, b: i32| -> i32 { a - b })?;
    engine.export("prim_sub", |_: &State, a: i64, b: i64| -> i64 { a - b })?;
    engine.export("prim_sub", |_: &State, a: f32, b: f32| -> f32 { a - b })?;
    engine.export("prim_sub", |_: &State, a: f64, b: f64| -> f64 { a - b })?;
    engine.export("prim_negate", |_: &State, a: i8| -> i8 { -a })?;
    engine.export("prim_negate", |_: &State, a: i16| -> i16 { -a })?;
    engine.export("prim_negate", |_: &State, a: i32| -> i32 { -a })?;
    engine.export("prim_negate", |_: &State, a: i64| -> i64 { -a })?;
    engine.export("prim_negate", |_: &State, a: f32| -> f32 { -a })?;
    engine.export("prim_negate", |_: &State, a: f64| -> f64 { -a })?;

    // Multiplication and division
    engine.export("prim_mul", |_: &State, a: u8, b: u8| -> u8 { a * b })?;
    engine.export("prim_mul", |_: &State, a: u16, b: u16| -> u16 { a * b })?;
    engine.export("prim_mul", |_: &State, a: u32, b: u32| -> u32 { a * b })?;
    engine.export("prim_mul", |_: &State, a: u64, b: u64| -> u64 { a * b })?;
    engine.export("prim_mul", |_: &State, a: i8, b: i8| -> i8 { a * b })?;
    engine.export("prim_mul", |_: &State, a: i16, b: i16| -> i16 { a * b })?;
    engine.export("prim_mul", |_: &State, a: i32, b: i32| -> i32 { a * b })?;
    engine.export("prim_mul", |_: &State, a: i64, b: i64| -> i64 { a * b })?;
    engine.export("prim_mul", |_: &State, a: f32, b: f32| -> f32 { a * b })?;
    engine.export("prim_mul", |_: &State, a: f64, b: f64| -> f64 { a * b })?;
    engine.export("prim_div", |_: &State, a: f32, b: f32| -> f32 { a / b })?;
    engine.export("prim_div", |_: &State, a: f64, b: f64| -> f64 { a / b })?;

    // Remainder
    engine.export("prim_mod", |_: &State, a: u8, b: u8| -> u8 { a % b })?;
    engine.export("prim_mod", |_: &State, a: u16, b: u16| -> u16 { a % b })?;
    engine.export("prim_mod", |_: &State, a: u32, b: u32| -> u32 { a % b })?;
    engine.export("prim_mod", |_: &State, a: u64, b: u64| -> u64 { a % b })?;
    engine.export("prim_mod", |_: &State, a: i8, b: i8| -> i8 { a % b })?;
    engine.export("prim_mod", |_: &State, a: i16, b: i16| -> i16 { a % b })?;
    engine.export("prim_mod", |_: &State, a: i32, b: i32| -> i32 { a % b })?;
    engine.export("prim_mod", |_: &State, a: i64, b: i64| -> i64 { a % b })?;

    // Numeric conversions (used by `std.json`).
    engine.export("prim_to_f64", |_: &State, x: u8| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: u16| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: u32| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: u64| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: i8| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: i16| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: i32| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: i64| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: f32| -> f64 { x as f64 })?;
    engine.export("prim_to_f64", |_: &State, x: f64| -> f64 { x })?;

    // f64 -> Option <number> conversions (used by `std.json`).
    // - reject NaN/±inf
    // - for integer types: require integral `x` (fract == 0) and in range
    {
        macro_rules! inject_f64_to {
            ($name:literal, $dst_ty:expr, $convert:expr) => {{
                let scheme = Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(Type::con("f64", 0), Type::option($dst_ty)),
                );
                engine.export_native($name, scheme, 1, move |engine, _t, args| {
                    let x = f64::from_pointer(engine.heap(), &args[0])?;
                    let converted: Option<Pointer> = $convert(engine, x)?;
                    option_from_pointer(engine.heap(), converted)
                })?;
            }};
        }

        inject_f64_to!("prim_f64_to_u8", Type::con("u8", 0), |engine: &Engine<
            State,
        >,
                                                              x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= u8::MIN as f64 && x <= u8::MAX as f64 {
                Ok(Some(engine.heap().alloc_u8(x as u8)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_u16", Type::con("u16", 0), |engine: &Engine<
            State,
        >,
                                                                x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= u16::MIN as f64 && x <= u16::MAX as f64 {
                Ok(Some(engine.heap().alloc_u16(x as u16)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_u32", Type::con("u32", 0), |engine: &Engine<
            State,
        >,
                                                                x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= u32::MIN as f64 && x <= u32::MAX as f64 {
                Ok(Some(engine.heap().alloc_u32(x as u32)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_u64", Type::con("u64", 0), |engine: &Engine<
            State,
        >,
                                                                x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= u64::MIN as f64 && x <= u64::MAX as f64 {
                Ok(Some(engine.heap().alloc_u64(x as u64)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_i8", Type::con("i8", 0), |engine: &Engine<
            State,
        >,
                                                              x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= i8::MIN as f64 && x <= i8::MAX as f64 {
                Ok(Some(engine.heap().alloc_i8(x as i8)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_i16", Type::con("i16", 0), |engine: &Engine<
            State,
        >,
                                                                x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= i16::MIN as f64 && x <= i16::MAX as f64 {
                Ok(Some(engine.heap().alloc_i16(x as i16)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_i32", Type::con("i32", 0), |engine: &Engine<
            State,
        >,
                                                                x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= i32::MIN as f64 && x <= i32::MAX as f64 {
                Ok(Some(engine.heap().alloc_i32(x as i32)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_i64", Type::con("i64", 0), |engine: &Engine<
            State,
        >,
                                                                x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x.fract() == 0.0 && x >= i64::MIN as f64 && x <= i64::MAX as f64 {
                Ok(Some(engine.heap().alloc_i64(x as i64)?))
            } else {
                Ok(None)
            }
        });
        inject_f64_to!("prim_f64_to_f32", Type::con("f32", 0), |engine: &Engine<
            State,
        >,
                                                                x: f64|
         -> Result<
            Option<Pointer>,
            EngineError,
        > {
            if x.is_finite() && x >= f32::MIN as f64 && x <= f32::MAX as f64 {
                Ok(Some(engine.heap().alloc_f32(x as f32)?))
            } else {
                Ok(None)
            }
        });
    }

    Ok(())
}

pub(crate) fn inject_json_primops<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    // List -> Array conversion.
    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let array_a = Type::array(a);
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(list_a, array_a));
        engine.export_native("prim_array_from_list", scheme, 1, |engine, _, args| {
            let values = expect_list(engine.heap(), &args[0])?;
            engine.heap().alloc_array(values)
        })?;
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
        engine.export_native_async("prim_dict_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let dict_ty = arg_tys[1].clone();
                let elem_ty = dict_elem_type(&dict_ty)?;
                let map = engine.heap().pointer_as_dict(&args[1])?;
                let func = args[0].clone();
                let mut out: BTreeMap<Symbol, Pointer> = BTreeMap::new();
                for (k, v) in &map {
                    let mapped = invoke_pointer_fn(
                        engine,
                        func.clone(),
                        v.clone(),
                        Some(&func_ty),
                        Some(&elem_ty),
                    )
                    .await?;
                    out.insert(k.clone(), mapped);
                }
                engine.heap().alloc_dict(out)
            }
            .boxed_local()
        })?;
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
        engine.export_native_async(
            "prim_dict_traverse_result",
            scheme,
            2,
            |engine, call_type, args| {
                async move {
                    let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                    let func_ty = arg_tys[0].clone();
                    let dict_ty = arg_tys[1].clone();
                    let elem_ty = dict_elem_type(&dict_ty)?;
                    let map = engine.heap().pointer_as_dict(&args[1])?;

                    let func = args[0].clone();
                    let mut out: BTreeMap<Symbol, Pointer> = BTreeMap::new();
                    for (k, v) in &map {
                        let mapped = invoke_pointer_fn(
                            engine,
                            func.clone(),
                            v.clone(),
                            Some(&func_ty),
                            Some(&elem_ty),
                        )
                        .await?;
                        match result_value(engine.heap(), &mapped)? {
                            Ok(ok) => {
                                out.insert(k.clone(), ok);
                            }
                            Err(err) => return result_from_pointer(engine.heap(), Err(err)),
                        }
                    }

                    let dict = engine.heap().alloc_dict(out)?;
                    result_from_pointer(engine.heap(), Ok(dict))
                }
                .boxed_local()
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
        engine.export_native("prim_parse_uuid", scheme, 1, |engine, _, args| {
            let s = String::from_pointer(engine.heap(), &args[0])?;
            let parsed = Uuid::parse_str(&s)
                .ok()
                .map(|uuid| engine.heap().alloc_uuid(uuid))
                .transpose()?;
            option_from_pointer(engine.heap(), parsed)
        })?;
    }

    {
        let string_ty = Type::con("string", 0);
        let dt_ty = Type::con("datetime", 0);
        let scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(string_ty.clone(), Type::option(dt_ty)),
        );
        engine.export_native("prim_parse_datetime", scheme, 1, |engine, _, args| {
            let s = String::from_pointer(engine.heap(), &args[0])?;
            let parsed = DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .map(|dt| engine.heap().alloc_datetime(dt))
                .transpose()?;
            option_from_pointer(engine.heap(), parsed)
        })?;
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

        fn to_serde_json(heap: &Heap, v: &Value, tags: &Tags) -> Option<serde_json::Value> {
            match v {
                Value::Adt(tag, _) if tag == &tags.null => Some(serde_json::Value::Null),
                Value::Adt(tag, args) if tag == &tags.bool_ => match args.as_slice() {
                    [arg] => heap
                        .get(arg)
                        .ok()?
                        .as_ref()
                        .value_as_bool()
                        .ok()
                        .map(serde_json::Value::Bool),
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.string => match args.as_slice() {
                    [arg] => heap
                        .get(arg)
                        .ok()?
                        .as_ref()
                        .value_as_string()
                        .ok()
                        .map(serde_json::Value::String),
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.number => match args.as_slice() {
                    [arg] => {
                        let n = heap.get(arg).ok()?.as_ref().value_as_f64().ok()?;
                        serde_json::Number::from_f64(n)
                            .map(serde_json::Value::Number)
                            .or(Some(serde_json::Value::Null))
                    }
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.array => match args.as_slice() {
                    [arg] => {
                        let xs = heap.get(arg).ok()?.as_ref().value_as_array().ok()?;
                        let mut out = Vec::with_capacity(xs.len());
                        for x in &xs {
                            let x_value = heap.get(x).ok()?;
                            out.push(to_serde_json(heap, x_value.as_ref(), tags)?);
                        }
                        Some(serde_json::Value::Array(out))
                    }
                    _ => None,
                },
                Value::Adt(tag, args) if tag == &tags.object => match args.as_slice() {
                    [arg] => {
                        let map = heap.get(arg).ok()?.as_ref().value_as_dict().ok()?;
                        let mut out = serde_json::Map::with_capacity(map.len());
                        for (k, v) in &map {
                            let v_value = heap.get(v).ok()?;
                            out.insert(
                                k.as_ref().to_string(),
                                to_serde_json(heap, v_value.as_ref(), tags)?,
                            );
                        }
                        Some(serde_json::Value::Object(out))
                    }
                    _ => None,
                },
                _ => None,
            }
        }

        engine.export_native("prim_json_stringify", scheme, 1, move |engine, _, args| {
            let value = engine.heap().get(&args[0])?;
            let Some(json) = to_serde_json(engine.heap(), value.as_ref(), &tags) else {
                return engine.heap().alloc_string("<non-std.json.Value>".into());
            };
            engine.heap().alloc_string(json.to_string())
        })?;
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
        ) -> Result<Pointer, EngineError> {
            match v {
                serde_json::Value::Null => heap.alloc_adt(tags.null.clone(), vec![]),
                serde_json::Value::Bool(b) => {
                    let value = heap.alloc_bool(*b)?;
                    heap.alloc_adt(tags.bool_.clone(), vec![value])
                }
                serde_json::Value::String(s) => {
                    let value = heap.alloc_string(s.clone())?;
                    heap.alloc_adt(tags.string.clone(), vec![value])
                }
                serde_json::Value::Number(n) => {
                    let Some(f) = n.as_f64() else {
                        return Err(EngineError::Custom(
                            "expected JSON number representable as f64".into(),
                        ));
                    };
                    let value = heap.alloc_f64(f)?;
                    heap.alloc_adt(tags.number.clone(), vec![value])
                }
                serde_json::Value::Array(xs) => {
                    let mut out = Vec::with_capacity(xs.len());
                    for x in xs {
                        out.push(to_json_value(x, tags, heap)?);
                    }
                    let array = heap.alloc_array(out)?;
                    heap.alloc_adt(tags.array.clone(), vec![array])
                }
                serde_json::Value::Object(obj) => {
                    let mut out = BTreeMap::new();
                    for (k, v) in obj {
                        let value = to_json_value(v, tags, heap)?;
                        out.insert(intern(k.as_str()), value);
                    }
                    let dict = heap.alloc_dict(out)?;
                    heap.alloc_adt(tags.object.clone(), vec![dict])
                }
            }
        }

        fn result_ok(heap: &Heap, v: Pointer) -> Result<Pointer, EngineError> {
            heap.alloc_adt(sym("Ok"), vec![v])
        }

        fn result_err(heap: &Heap, msg: String) -> Result<Pointer, EngineError> {
            let msg = heap.alloc_string(msg)?;
            heap.alloc_adt(sym("Err"), vec![msg])
        }

        engine.export_native("prim_json_parse", scheme, 1, move |engine, _, args| {
            let s = String::from_pointer(engine.heap(), &args[0])?;
            let parsed: serde_json::Value = match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(e) => return result_err(engine.heap(), e.to_string()),
            };
            match to_json_value(&parsed, &tags, engine.heap()) {
                Ok(v) => result_ok(engine.heap(), v),
                Err(err) => result_err(engine.heap(), err.to_string()),
            }
        })?;
    }

    Ok(())
}

pub(crate) fn inject_list_builtins<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
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
        engine.export_native_async("prim_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[1])?;
                let out = map_values(engine, args[0].clone(), &func_ty, &elem_ty, values).await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                list_from_pointers(engine.heap(), ptrs)
            }
            .boxed_local()
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
        engine.export_native_async("prim_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[1])?;
                let out = map_values(engine, args[0].clone(), &func_ty, &elem_ty, values).await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                engine.heap().alloc_array(ptrs)
            }
            .boxed_local()
        })?;
    }

    {
        let a_tv = engine.fresh_type_var(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(a, array_a));
        engine.export_native("prim_array_singleton", scheme, 1, |engine, _, args| {
            let ptr = args[0].clone();
            engine.heap().alloc_array(vec![ptr])
        })?;
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
        engine.export_native_async("prim_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                let func = args[0].clone();
                match option_value(engine.heap(), &args[1])? {
                    Some(v) => {
                        let mapped =
                            invoke_pointer_fn(engine, func, v, Some(&func_ty), Some(&elem_ty))
                                .await?;
                        option_from_pointer(engine.heap(), Some(mapped))
                    }
                    None => option_from_pointer(engine.heap(), None),
                }
            }
            .boxed_local()
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
        engine.export_native_async("prim_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let result_ty = arg_tys[1].clone();
                let (ok_ty, _err_ty) = result_types(&result_ty)?;
                let func = args[0].clone();
                let result = args[1].clone();
                match result_value(engine.heap(), &result)? {
                    Ok(v) => {
                        let mapped =
                            invoke_pointer_fn(engine, func, v, Some(&func_ty), Some(&ok_ty))
                                .await?;
                        result_from_pointer(engine.heap(), Ok(mapped))
                    }
                    Err(e) => result_from_pointer(engine.heap(), Err(e)),
                }
            }
            .boxed_local()
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
        engine.export_native_async("prim_foldl", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let list_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[2])?;
                foldl_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    values,
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_foldl", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let array_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[2])?;
                foldl_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    values,
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_foldl", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let opt_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = option_elem_type(&opt_ty)?;
                foldl_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    option_value(engine.heap(), &args[2])?.into_iter(),
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_foldr", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let list_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[2])?;
                foldr_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    values,
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_foldr", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let array_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[2])?;
                foldr_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    values,
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_foldr", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let opt_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = option_elem_type(&opt_ty)?;
                foldr_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    option_value(engine.heap(), &args[2])?.into_iter().collect(),
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_fold", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let list_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[2])?;
                foldl_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    values,
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_fold", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let array_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[2])?;
                foldl_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    values,
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_fold", scheme, 3, |engine, call_type, args| {
            async move {
                let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
                let func_ty = arg_tys[0].clone();
                let acc_ty = arg_tys[1].clone();
                let opt_ty = arg_tys[2].clone();
                if acc_ty != res_ty {
                    return Err(EngineError::NativeType {
                        expected: acc_ty.to_string(),
                        got: res_ty.to_string(),
                    });
                }
                let elem_ty = option_elem_type(&opt_ty)?;
                foldl_values(
                    engine,
                    args[0].clone(),
                    &func_ty,
                    &acc_ty,
                    &elem_ty,
                    args[1].clone(),
                    option_value(engine.heap(), &args[2])?.into_iter(),
                )
                .await
            }
            .boxed_local()
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
        engine.export_native_async("prim_filter", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[1])?;
                let out =
                    filter_values(engine, args[0].clone(), &func_ty, &elem_ty, values).await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                list_from_pointers(engine.heap(), ptrs)
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_filter", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[1])?;
                let out =
                    filter_values(engine, args[0].clone(), &func_ty, &elem_ty, values).await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                engine.heap().alloc_array(ptrs)
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_filter", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                let func = args[0].clone();
                match option_value(engine.heap(), &args[1])? {
                    Some(v) => {
                        let keep =
                            invoke_pointer_fn(engine, func, v, Some(&func_ty), Some(&elem_ty))
                                .await?;
                        if bool::from_pointer(engine.heap(), &keep)? {
                            Ok(args[1].clone())
                        } else {
                            option_from_pointer(engine.heap(), None)
                        }
                    }
                    None => option_from_pointer(engine.heap(), None),
                }
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_filter_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[1])?;
                let out =
                    filter_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values).await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                list_from_pointers(engine.heap(), ptrs)
            }
            .boxed_local()
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
                Type::fun(a.clone(), Type::option(b.clone())),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.export_native_async("prim_filter_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[1])?;
                let out =
                    filter_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values).await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                engine.heap().alloc_array(ptrs)
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_filter_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                let func = args[0].clone();
                match option_value(engine.heap(), &args[1])? {
                    Some(v) => {
                        let mapped =
                            invoke_pointer_fn(engine, func, v, Some(&func_ty), Some(&elem_ty))
                                .await?;
                        Ok(mapped)
                    }
                    None => option_from_pointer(engine.heap(), None),
                }
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_flat_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[1])?;
                let out =
                    flat_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values, |v| {
                        let mapped = engine.heap().get(v)?;
                        list_to_vec(engine.heap(), mapped.as_ref())
                    })
                    .await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                list_from_pointers(engine.heap(), ptrs)
            }
            .boxed_local()
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
                Type::fun(a.clone(), array_b.clone()),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.export_native_async("prim_flat_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[1])?;
                let out =
                    flat_map_values(engine, args[0].clone(), &func_ty, &elem_ty, values, |v| {
                        expect_array(engine.heap(), v)
                    })
                    .await?;
                let ptrs = values_to_ptrs(engine.heap(), out)?;
                engine.heap().alloc_array(ptrs)
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_flat_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                let func = args[0].clone();
                match option_value(engine.heap(), &args[1])? {
                    Some(v) => {
                        invoke_pointer_fn(engine, func, v, Some(&func_ty), Some(&elem_ty)).await
                    }
                    None => option_from_pointer(engine.heap(), None),
                }
            }
            .boxed_local()
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
                Type::fun(a.clone(), result_b.clone()),
                Type::fun(result_a.clone(), result_b),
            ),
        );
        engine.export_native_async("prim_flat_map", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let result_ty = arg_tys[1].clone();
                let (ok_ty, _err_ty) = result_types(&result_ty)?;
                let func = args[0].clone();
                let result = args[1].clone();
                match result_value(engine.heap(), &result)? {
                    Ok(v) => {
                        let mapped =
                            invoke_pointer_fn(engine, func, v, Some(&func_ty), Some(&ok_ty))
                                .await?;
                        let _ = result_value(engine.heap(), &mapped)?;
                        Ok(mapped)
                    }
                    Err(e) => result_from_pointer(engine.heap(), Err(e)),
                }
            }
            .boxed_local()
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
                Type::fun(list_a.clone(), list_a.clone()),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.export_native_async("prim_or_else", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                if !expect_list(engine.heap(), &args[1])?.is_empty() {
                    return Ok(args[1].clone());
                }
                let func = args[0].clone();
                let list = args[1].clone();
                invoke_pointer_fn(engine, func, list, Some(&func_ty), Some(&list_ty)).await
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_or_else", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                if !expect_array(engine.heap(), &args[1])?.is_empty() {
                    return Ok(args[1].clone());
                }
                let func = args[0].clone();
                let array = args[1].clone();
                invoke_pointer_fn(engine, func, array, Some(&func_ty), Some(&array_ty)).await
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_or_else", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                if option_value(engine.heap(), &args[1])?.is_some() {
                    return Ok(args[1].clone());
                }
                let func = args[0].clone();
                let opt = args[1].clone();
                invoke_pointer_fn(engine, func, opt, Some(&func_ty), Some(&opt_ty)).await
            }
            .boxed_local()
        })?;
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
        engine.export_native_async("prim_or_else", scheme, 2, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let result_ty = arg_tys[1].clone();
                let result = args[1].clone();
                if result_value(engine.heap(), &result)?.is_err() {
                    let func = args[0].clone();
                    invoke_pointer_fn(engine, func, result, Some(&func_ty), Some(&result_ty)).await
                } else {
                    Ok(args[1].clone())
                }
            }
            .boxed_local()
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
        engine.export_native_async("sum", scheme, 1, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
                let list_ty = arg_tys[0].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let mut values = expect_list(engine.heap(), &args[0])?;
                if values.is_empty() {
                    return engine.resolve_global(&sym("zero"), &elem_ty).await;
                }
                let plus = resolve_binary_op(engine, "+", &elem_ty).await?;
                let plus_ty =
                    Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
                let acc = values.remove(0);
                foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc, values).await
            }
            .boxed_local()
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
        engine.export_native_async("sum", scheme, 1, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
                let array_ty = arg_tys[0].clone();
                let elem_ty = array_elem_type(&array_ty)?;
                let mut values = expect_array(engine.heap(), &args[0])?;
                if values.is_empty() {
                    return engine.resolve_global(&sym("zero"), &elem_ty).await;
                }
                let plus = resolve_binary_op(engine, "+", &elem_ty).await?;
                let plus_ty =
                    Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
                let acc = values.remove(0);
                foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc, values).await
            }
            .boxed_local()
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
        engine.export_native_async("sum", scheme, 1, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
                let opt_ty = arg_tys[0].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                match option_value(engine.heap(), &args[0])? {
                    Some(v) => Ok(v),
                    None => engine.resolve_global(&sym("zero"), &elem_ty).await,
                }
            }
            .boxed_local()
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
        engine.export_native_async("mean", scheme, 1, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
                let list_ty = arg_tys[0].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let mut values = expect_list(engine.heap(), &args[0])?;
                let len = values.len();
                if len == 0 {
                    return Err(EngineError::EmptySequence);
                }
                let plus = resolve_binary_op(engine, "+", &elem_ty).await?;
                let div = resolve_binary_op(engine, "/", &elem_ty).await?;
                let plus_ty =
                    Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
                let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
                let acc0 = values.remove(0);
                let acc =
                    foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc0, values).await?;
                let len_val = len_value_for_type(engine.heap(), &elem_ty, len)?;
                let div_step =
                    invoke_pointer_fn(engine, div, acc, Some(&plus_ty), Some(&elem_ty)).await?;
                invoke_pointer_fn(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty)).await
            }
            .boxed_local()
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
        engine.export_native_async("mean", scheme, 1, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
                let array_ty = arg_tys[0].clone();
                let elem_ty = array_elem_type(&array_ty)?;
                let mut values = expect_array(engine.heap(), &args[0])?;
                let len = values.len();
                if len == 0 {
                    return Err(EngineError::EmptySequence);
                }
                let plus = resolve_binary_op(engine, "+", &elem_ty).await?;
                let div = resolve_binary_op(engine, "/", &elem_ty).await?;
                let plus_ty =
                    Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
                let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
                let acc0 = values.remove(0);
                let acc =
                    foldl_values(engine, plus, &plus_ty, &elem_ty, &elem_ty, acc0, values).await?;
                let len_val = len_value_for_type(engine.heap(), &elem_ty, len)?;
                let div_step =
                    invoke_pointer_fn(engine, div, acc, Some(&plus_ty), Some(&elem_ty)).await?;
                invoke_pointer_fn(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty)).await
            }
            .boxed_local()
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
        engine.export_native_async("mean", scheme, 1, |engine, call_type, args| {
            async move {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
                let opt_ty = arg_tys[0].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                match option_value(engine.heap(), &args[0])? {
                    Some(v) => {
                        let len_val = len_value_for_type(engine.heap(), &elem_ty, 1)?;
                        let div = resolve_binary_op(engine, "/", &elem_ty).await?;
                        let div_ty =
                            Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
                        let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
                        let div_step =
                            invoke_pointer_fn(engine, div, v, Some(&div_ty), Some(&elem_ty))
                                .await?;
                        invoke_pointer_fn(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty))
                            .await
                    }
                    None => Err(EngineError::EmptySequence),
                }
            }
            .boxed_local()
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
        engine.export_native("count", scheme, 1, |engine, _, args| {
            engine
                .heap()
                .alloc_i32(expect_list(engine.heap(), &args[0])?.len() as i32)
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
        engine.export_native("count", scheme, 1, |engine, _, args| {
            engine
                .heap()
                .alloc_i32(expect_array(engine.heap(), &args[0])?.len() as i32)
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
        engine.export_native("count", scheme, 1, |engine, _, args| {
            engine
                .heap()
                .alloc_i32(option_value(engine.heap(), &args[0])?.is_some() as i32)
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
        engine.export_native("prim_take", scheme, 2, |engine, _, args| {
            let n_ptr = args[0].clone();
            let n = i32::from_pointer(engine.heap(), &n_ptr)?;
            let n = as_nonneg_usize(n);
            let xs = expect_list(engine.heap(), &args[1])?;
            list_from_pointers(engine.heap(), xs.into_iter().take(n).collect())
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
        engine.export_native("prim_take", scheme, 2, |engine, _, args| {
            let n_ptr = args[0].clone();
            let n = i32::from_pointer(engine.heap(), &n_ptr)?;
            let n = as_nonneg_usize(n);
            let xs = expect_array(engine.heap(), &args[1])?;
            let ptrs = values_to_ptrs(engine.heap(), xs.into_iter().take(n).collect())?;
            engine.heap().alloc_array(ptrs)
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
        engine.export_native("prim_skip", scheme, 2, |engine, _, args| {
            let n_ptr = args[0].clone();
            let n = i32::from_pointer(engine.heap(), &n_ptr)?;
            let n = as_nonneg_usize(n);
            let xs = expect_list(engine.heap(), &args[1])?;
            list_from_pointers(engine.heap(), xs.into_iter().skip(n).collect())
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
        engine.export_native("prim_skip", scheme, 2, |engine, _, args| {
            let n_ptr = args[0].clone();
            let n = i32::from_pointer(engine.heap(), &n_ptr)?;
            let n = as_nonneg_usize(n);
            let xs = expect_array(engine.heap(), &args[1])?;
            let ptrs = values_to_ptrs(engine.heap(), xs.into_iter().skip(n).collect())?;
            engine.heap().alloc_array(ptrs)
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
        engine.export_native("prim_get", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let list_ty = arg_tys[1].clone();
            let _elem_ty = list_elem_type(&list_ty)?;
            let idx_ptr = args[0].clone();
            let idx = i32::from_pointer(engine.heap(), &idx_ptr)?;
            let xs = expect_list(engine.heap(), &args[1])?;
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
        engine.export_native("prim_get", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let array_ty = arg_tys[1].clone();
            let _elem_ty = array_elem_type(&array_ty)?;
            let idx_ptr = args[0].clone();
            let idx = i32::from_pointer(engine.heap(), &idx_ptr)?;
            let xs = expect_array(engine.heap(), &args[1])?;
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
        engine.export_native("prim_get", scheme, 2, move |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let tuple_ty = arg_tys[1].clone();
            let _elem_ty = tuple_elem_type(&tuple_ty)?;
            let idx_ptr = args[0].clone();
            let idx = i32::from_pointer(engine.heap(), &idx_ptr)?;
            let idx_usize = checked_index(sym("prim_get"), idx, size)?;
            let xs = engine.heap().pointer_as_tuple(&args[1])?;
            if xs.len() != size {
                return Err(EngineError::NativeType {
                    expected: format!("tuple{}", size),
                    got: format!("tuple{}", xs.len()),
                });
            }
            Ok(xs[idx_usize].clone())
        })?;
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
        engine.export_native("prim_zip", scheme, 2, |engine, _, args| {
            let xs = expect_list(engine.heap(), &args[0])?;
            let ys = expect_list(engine.heap(), &args[1])?;
            let zipped = zip_tuple2(engine.heap(), xs, ys)?;
            list_from_pointers(engine.heap(), zipped)
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
        engine.export_native("prim_zip", scheme, 2, |engine, _, args| {
            let xs = expect_array(engine.heap(), &args[0])?;
            let ys = expect_array(engine.heap(), &args[1])?;
            let zipped = zip_tuple2(engine.heap(), xs, ys)?;
            let ptrs = values_to_ptrs(engine.heap(), zipped)?;
            engine.heap().alloc_array(ptrs)
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
        engine.export_native("prim_unzip", scheme, 1, |engine, _, args| {
            let pairs = expect_list(engine.heap(), &args[0])?;
            let (left, right) = unzip_tuple2(engine.heap(), pairs)?;
            let left = list_from_pointers(engine.heap(), left)?;
            let right = list_from_pointers(engine.heap(), right)?;
            engine.heap().alloc_tuple(vec![left, right])
        })?;
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
        engine.export_native("prim_unzip", scheme, 1, |engine, _, args| {
            let pairs = expect_array(engine.heap(), &args[0])?;
            let (left, right) = unzip_tuple2(engine.heap(), pairs)?;
            let left = engine
                .heap()
                .alloc_array(values_to_ptrs(engine.heap(), left)?)?;
            let right = engine
                .heap()
                .alloc_array(values_to_ptrs(engine.heap(), right)?)?;
            engine.heap().alloc_tuple(vec![left, right])
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
        engine.export_native("min", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let list = engine.heap().get(&args[0])?;
            let values = list_to_vec(engine.heap(), list.as_ref())?;
            extremum_by_type(
                engine.heap(),
                "min",
                &elem_ty,
                values,
                std::cmp::Ordering::Less,
            )
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
        engine.export_native("min", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty)?;
            let values = expect_array(engine.heap(), &args[0])?;
            extremum_by_type(
                engine.heap(),
                "min",
                &elem_ty,
                values,
                std::cmp::Ordering::Less,
            )
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
        engine.export_native("min", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty)?;
            match option_value(engine.heap(), &args[0])? {
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
        engine.export_native("max", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let list = engine.heap().get(&args[0])?;
            let values = list_to_vec(engine.heap(), list.as_ref())?;
            extremum_by_type(
                engine.heap(),
                "max",
                &elem_ty,
                values,
                std::cmp::Ordering::Greater,
            )
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
        engine.export_native("max", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty)?;
            let values = expect_array(engine.heap(), &args[0])?;
            extremum_by_type(
                engine.heap(),
                "max",
                &elem_ty,
                values,
                std::cmp::Ordering::Greater,
            )
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
        engine.export_native("max", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty)?;
            match option_value(engine.heap(), &args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    Ok(())
}

pub(crate) fn inject_option_result_builtins<State: Clone + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    let is_some = sym("is_some");
    let is_some_scheme = engine.lookup_scheme(&is_some)?;
    engine.export_native("is_some", is_some_scheme, 1, |engine, _, args| {
        engine
            .heap()
            .alloc_bool(option_value(engine.heap(), &args[0])?.is_some())
    })?;
    let is_none = sym("is_none");
    let is_none_scheme = engine.lookup_scheme(&is_none)?;
    engine.export_native("is_none", is_none_scheme, 1, |engine, _, args| {
        engine
            .heap()
            .alloc_bool(option_value(engine.heap(), &args[0])?.is_none())
    })?;

    let is_ok = sym("is_ok");
    let is_ok_scheme = engine.lookup_scheme(&is_ok)?;
    engine.export_native("is_ok", is_ok_scheme, 1, |engine, _, args| {
        engine
            .heap()
            .alloc_bool(result_value(engine.heap(), &args[0])?.is_ok())
    })?;
    let is_err = sym("is_err");
    let is_err_scheme = engine.lookup_scheme(&is_err)?;
    engine.export_native("is_err", is_err_scheme, 1, |engine, _, args| {
        engine
            .heap()
            .alloc_bool(result_value(engine.heap(), &args[0])?.is_err())
    })?;
    Ok(())
}
