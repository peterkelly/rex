//! Prelude injection helpers for Rex.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rex_ast::expr::Symbol;
use rex_typesystem::{
    types::{BuiltinTypeId, Scheme, Type, TypeKind, Types},
    unification::unify,
};
use uuid::Uuid;

use crate::Engine;
use crate::EngineError;
use crate::engine::{SchedulerNativeResult, binary_arg_types};
use crate::stack::{
    NativeApplyUnary, NativeArrayEq, NativeArrayEqState, NativeDictMap, NativeDictTraverseResult,
    NativeFold, NativeFoldOrder, NativeFoldState, NativeMean, NativeMeanState,
    NativeSequenceFilter, NativeSequenceFilterMap, NativeSequenceFlatMap, NativeSequenceMap,
    NativeSequenceShape, NativeSum, NativeTask, NativeUnaryFilter, NativeUnaryFilterMap,
    NativeUnaryFlatMap, NativeUnaryMap, NativeUnaryShape,
};
use crate::value::{Cell, Handle, Heap, HeapAccess, Pointer};

fn expect_list(heap: &Heap, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
    heap.pointer_as_list(pointer)
}

fn list_from_handles(heap: &Heap, values: Vec<Handle>) -> Result<Handle, EngineError> {
    heap.alloc_list(values)
}

fn option_from_handle(heap: &Heap, value: Option<Handle>) -> Result<Handle, EngineError> {
    match value {
        Some(value) => heap.alloc_adt(Symbol::intern("Some"), vec![value]),
        None => heap.alloc_adt(Symbol::intern("None"), vec![]),
    }
}

fn option_handle(value: &Handle) -> Result<Option<Handle>, EngineError> {
    let (tag, args) = value.as_adt()?;
    if tag.as_ref() == "Some" && args.len() == 1 {
        Ok(Some(args[0].clone()))
    } else if tag.as_ref() == "None" && args.is_empty() {
        Ok(None)
    } else {
        Err(EngineError::NativeType {
            expected: "Option".into(),
            got: value.type_name()?.into(),
        })
    }
}

fn integer_overflow(typ: &'static str) -> EngineError {
    EngineError::from(format!("integer overflow ({typ})"))
}

fn integer_underflow(typ: &'static str) -> EngineError {
    EngineError::from(format!("integer underflow ({typ})"))
}

fn checked_integer_error(value: i128, min: i128, max: i128, typ: &'static str) -> EngineError {
    if value < min {
        integer_underflow(typ)
    } else {
        debug_assert!(value > max);
        integer_overflow(typ)
    }
}

fn result_handle(value: &Handle) -> Result<Result<Handle, Handle>, EngineError> {
    let (tag, args) = value.as_adt()?;
    if tag.as_ref() == "Ok" && args.len() == 1 {
        Ok(Ok(args[0].clone()))
    } else if tag.as_ref() == "Err" && args.len() == 1 {
        Ok(Err(args[0].clone()))
    } else {
        Err(EngineError::NativeType {
            expected: "Result".into(),
            got: value.type_name()?.into(),
        })
    }
}

pub(crate) fn list_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::List)) => {
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
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Array)) => {
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
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Dict)) => {
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
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Option)) => {
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
            TypeKind::App(head, err) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Result)) => {
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

pub(crate) fn expect_array(heap: &Heap, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
    heap.pointer_as_array(pointer)
}

pub(crate) fn option_from_pointer(
    heap: &Heap,
    value: Option<Pointer>,
) -> Result<Pointer, EngineError> {
    match value {
        Some(v) => heap.alloc_ptr_adt(Symbol::intern("Some"), vec![v]),
        None => heap.alloc_ptr_adt(Symbol::intern("None"), vec![]),
    }
}

pub(crate) fn option_value(heap: &Heap, pointer: &Pointer) -> Result<Option<Pointer>, EngineError> {
    let (tag, args) = heap.pointer_as_adt(pointer)?;
    if tag.as_ref() == "Some" && args.len() == 1 {
        Ok(Some(args[0]))
    } else if tag.as_ref() == "None" && args.is_empty() {
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
    if tag.as_ref() == "Ok" && args.len() == 1 {
        Ok(Ok(args[0]))
    } else if tag.as_ref() == "Err" && args.len() == 1 {
        Ok(Err(args[0]))
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
        Ok(v) => heap.alloc_ptr_adt(Symbol::intern("Ok"), vec![v]),
        Err(v) => heap.alloc_ptr_adt(Symbol::intern("Err"), vec![v]),
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

pub(crate) fn extremum_handle_by_type(
    heap: &Heap,
    name: &'static str,
    elem_ty: &Type,
    values: Vec<Handle>,
    choose: std::cmp::Ordering,
) -> Result<Handle, EngineError> {
    let name = Symbol::intern(name);
    let mut values = values.into_iter();
    let mut best = values.next().ok_or(EngineError::EmptySequence)?;
    for value in values {
        let value_ptr = value.pointer()?;
        let best_ptr = best.pointer()?;
        let ord = heap.with_access(|heap| {
            let cell = heap.get(&value_ptr)?;
            let best_cell = heap.get(&best_ptr)?;
            cmp_cell_by_type(&name, elem_ty, cell, best_cell)
        })?;
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

pub(crate) fn zip_tuple2_handles(
    heap: &Heap,
    xs: Vec<Handle>,
    ys: Vec<Handle>,
) -> Result<Vec<Handle>, EngineError> {
    xs.into_iter()
        .zip(ys)
        .map(|(x, y)| heap.alloc_tuple(vec![x, y]))
        .collect()
}

pub(crate) fn unzip_tuple2_handles(
    pairs: Vec<Handle>,
) -> Result<(Vec<Handle>, Vec<Handle>), EngineError> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for pair in pairs {
        let elems = pair.as_tuple()?;
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

fn cmp_cell_by_type(
    op_name: &Symbol,
    typ: &Type,
    lhs: &Cell,
    rhs: &Cell,
) -> Result<std::cmp::Ordering, EngineError> {
    fn mismatch(op_name: &Symbol, expected: &str, lhs: &Cell, rhs: &Cell) -> EngineError {
        let _ = op_name;
        EngineError::NativeType {
            expected: expected.to_string(),
            got: format!("{}, {}", lhs.cell_type_name(), rhs.cell_type_name()),
        }
    }

    match typ.as_ref() {
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::U8) => {
                let a = lhs
                    .cell_as_u8()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_u8()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::U16) => {
                let a = lhs
                    .cell_as_u16()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_u16()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::U32) => {
                let a = lhs
                    .cell_as_u32()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_u32()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::U64) => {
                let a = lhs
                    .cell_as_u64()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_u64()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::I8) => {
                let a = lhs
                    .cell_as_i8()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_i8()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::I16) => {
                let a = lhs
                    .cell_as_i16()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_i16()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::I32) => {
                let a = lhs
                    .cell_as_i32()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_i32()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::I64) => {
                let a = lhs
                    .cell_as_i64()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_i64()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::F32) => {
                let a = lhs
                    .cell_as_f32()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_f32()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                    expected: tc.name_str().to_string(),
                    got: "nan".into(),
                })
            }
            Some(BuiltinTypeId::F64) => {
                let a = lhs
                    .cell_as_f64()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_f64()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                    expected: tc.name_str().to_string(),
                    got: "nan".into(),
                })
            }
            Some(BuiltinTypeId::String) => {
                let a = lhs
                    .cell_as_string()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_string()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::Uuid) => {
                let a = lhs
                    .cell_as_uuid()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_uuid()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            Some(BuiltinTypeId::DateTime) => {
                let a = lhs
                    .cell_as_datetime()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                let b = rhs
                    .cell_as_datetime()
                    .map_err(|_| mismatch(op_name, tc.name_str(), lhs, rhs))?;
                Ok(a.cmp(&b))
            }
            _ => Err(mismatch(op_name, tc.name_str(), lhs, rhs)),
        },
        _ => Err(mismatch(op_name, &typ.to_string(), lhs, rhs)),
    }
}

pub(crate) fn inject_prelude_adts<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    let mut list_adt = engine.adt_decl("List", &["a"]);
    let a_name = Symbol::intern("a");
    let a = list_adt
        .param_type(&a_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("List")))?;
    let list_a = list_adt.result_type();
    list_adt.add_variant(Symbol::intern("Empty"), vec![]);
    list_adt.add_variant(Symbol::intern("Cons"), vec![a, list_a]);
    engine.inject_adt(list_adt)?;

    let mut option_adt = engine.adt_decl("Option", &["t"]);
    let t_name = Symbol::intern("t");
    let t = option_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("Option")))?;
    option_adt.add_variant(Symbol::intern("Some"), vec![t]);
    option_adt.add_variant(Symbol::intern("None"), vec![]);
    engine.inject_adt(option_adt)?;

    let mut result_adt = engine.adt_decl("Result", &["e", "t"]);
    let e_name = Symbol::intern("e");
    let t_name = Symbol::intern("t");
    let e = result_adt
        .param_type(&e_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("Result")))?;
    let t = result_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("Result")))?;
    result_adt.add_variant(Symbol::intern("Err"), vec![e]);
    result_adt.add_variant(Symbol::intern("Ok"), vec![t]);
    engine.inject_adt(result_adt)?;
    Ok(())
}

pub(crate) fn inject_equality_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    // Equality primitives are monomorphic overloads (same name, different
    // concrete types), matching the numeric `prim_add` style.
    engine.export("prim_eq", |_: &State, a: bool, b: bool| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: bool, b: bool| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: u8, b: u8| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u8, b: u8| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: u16, b: u16| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u16, b: u16| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: u32, b: u32| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u32, b: u32| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: u64, b: u64| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u64, b: u64| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: i8, b: i8| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i8, b: i8| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: i16, b: i16| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i16, b: i16| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: i32, b: i32| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i32, b: i32| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: i64, b: i64| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i64, b: i64| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: f32, b: f32| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: f32, b: f32| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: f64, b: f64| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: f64, b: f64| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: String, b: String| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: String, b: String| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: Uuid, b: Uuid| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: Uuid, b: Uuid| Ok(a != b))?;
    engine.export(
        "prim_eq",
        |_: &State, a: DateTime<Utc>, b: DateTime<Utc>| Ok(a == b),
    )?;
    engine.export(
        "prim_ne",
        |_: &State, a: DateTime<Utc>, b: DateTime<Utc>| Ok(a != b),
    )?;

    // Array equality must respect `Eq a`. We can't express the loop without a
    // primitive, but we *can* express the element comparison: the primitive
    // calls `(==)` on each pair.
    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::app(Type::builtin(BuiltinTypeId::Array), a);
        let bool_ty = Type::builtin(BuiltinTypeId::Bool);
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_a.clone(), bool_ty.clone())),
        );
        engine.export_native_scheduler(
            "prim_array_eq",
            scheme.clone(),
            2,
            |engine, call_type, args| {
                let (lhs_ty, rhs_ty) = binary_arg_types(&call_type)?;
                let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
                    expected: lhs_ty.to_string(),
                    got: rhs_ty.to_string(),
                })?;
                let array_ty = lhs_ty.apply(&subst);
                let elem_ty = array_elem_type(&array_ty)?;
                let xs = expect_array(engine.heap(), &args[0])?;
                let ys = expect_array(engine.heap(), &args[1])?;
                Ok(SchedulerNativeResult::Task(NativeTask::ArrayEq(
                    NativeArrayEq {
                        elem_type: elem_ty,
                        xs,
                        ys,
                        state: NativeArrayEqState::Enter,
                        next_index: 0,
                        step: None,
                        negate: false,
                    },
                )))
            },
        )?;

        let scheme = Scheme::new(
            vec![a_tv],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_a, bool_ty.clone())),
        );
        engine.export_native_scheduler("prim_array_ne", scheme, 2, |engine, call_type, args| {
            let (lhs_ty, rhs_ty) = binary_arg_types(&call_type)?;
            let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
                expected: lhs_ty.to_string(),
                got: rhs_ty.to_string(),
            })?;
            let array_ty = lhs_ty.apply(&subst);
            let elem_ty = array_elem_type(&array_ty)?;
            let xs = expect_array(engine.heap(), &args[0])?;
            let ys = expect_array(engine.heap(), &args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::ArrayEq(
                NativeArrayEq {
                    elem_type: elem_ty,
                    xs,
                    ys,
                    state: NativeArrayEqState::Enter,
                    next_index: 0,
                    step: None,
                    negate: true,
                },
            )))
        })?;
    }

    Ok(())
}

pub(crate) fn inject_order_ops<State: Clone + Send + Sync + 'static>(
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
    engine.export("prim_lt", |_: &State, a: u8, b: u8| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u8, b: u8| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u8, b: u8| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u8, b: u8| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u8, b: u8| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: u16, b: u16| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u16, b: u16| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u16, b: u16| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u16, b: u16| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u16, b: u16| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: u32, b: u32| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u32, b: u32| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u32, b: u32| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u32, b: u32| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u32, b: u32| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: u64, b: u64| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u64, b: u64| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u64, b: u64| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u64, b: u64| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u64, b: u64| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i8, b: i8| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i8, b: i8| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i8, b: i8| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i8, b: i8| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i8, b: i8| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i16, b: i16| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i16, b: i16| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i16, b: i16| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i16, b: i16| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i16, b: i16| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i32, b: i32| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i32, b: i32| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i32, b: i32| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i32, b: i32| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i32, b: i32| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i64, b: i64| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i64, b: i64| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i64, b: i64| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i64, b: i64| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i64, b: i64| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: String, b: String| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: String, b: String| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: String, b: String| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: String, b: String| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: String, b: String| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    // Floats: preserve the existing “NaN is a type error” semantics.
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let i32_ty = Type::builtin(BuiltinTypeId::I32);

    let f32_ty = Type::builtin(BuiltinTypeId::F32);
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
            let a = args[0].as_f32()?;
            let b = args[1].as_f32()?;
            let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                expected: "f32".into(),
                got: "nan".into(),
            })?;
            engine.heap().alloc_bool(pred(ord))
        })?;
    }
    engine.export_native("prim_cmp", f32_cmp, 2, |engine, _, args| {
        let a = args[0].as_f32()?;
        let b = args[1].as_f32()?;
        let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
            expected: "f32".into(),
            got: "nan".into(),
        })?;
        engine.heap().alloc_i32(cmp_to_i32(ord))
    })?;

    let f64_ty = Type::builtin(BuiltinTypeId::F64);
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
            let a = args[0].as_f64()?;
            let b = args[1].as_f64()?;
            let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                expected: "f64".into(),
                got: "nan".into(),
            })?;
            engine.heap().alloc_bool(pred(ord))
        })?;
    }
    engine.export_native("prim_cmp", f64_cmp, 2, |engine, _, args| {
        let a = args[0].as_f64()?;
        let b = args[1].as_f64()?;
        let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
            expected: "f64".into(),
            got: "nan".into(),
        })?;
        engine.heap().alloc_i32(cmp_to_i32(ord))
    })?;

    Ok(())
}

pub(crate) fn inject_show_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    engine.export("prim_show", |_: &State, x: bool| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u8| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u16| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u32| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u64| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i8| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i16| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i32| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i64| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: f32| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: f64| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: String| Ok(x))?;
    engine.export("prim_show", |_: &State, x: Uuid| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: DateTime<Utc>| Ok(x.to_string()))?;
    Ok(())
}

pub(crate) fn inject_boolean_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    engine.export("(&&)", |_: &State, a: bool, b: bool| Ok(a && b))?;
    engine.export("(||)", |_: &State, a: bool, b: bool| Ok(a || b))?;
    Ok(())
}

pub(crate) fn inject_numeric_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    macro_rules! export_checked_unsigned_add {
        ($ty:ty) => {
            engine.export("prim_add", |_: &State, a: $ty, b: $ty| {
                a.checked_add(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_signed_add {
        ($ty:ty) => {
            engine.export("prim_add", |_: &State, a: $ty, b: $ty| {
                a.checked_add(b).ok_or_else(|| {
                    checked_integer_error(
                        a as i128 + b as i128,
                        <$ty>::MIN as i128,
                        <$ty>::MAX as i128,
                        stringify!($ty),
                    )
                })
            })?;
        };
    }
    macro_rules! export_checked_unsigned_sub {
        ($ty:ty) => {
            engine.export("prim_sub", |_: &State, a: $ty, b: $ty| {
                a.checked_sub(b)
                    .ok_or_else(|| integer_underflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_signed_sub {
        ($ty:ty) => {
            engine.export("prim_sub", |_: &State, a: $ty, b: $ty| {
                a.checked_sub(b).ok_or_else(|| {
                    checked_integer_error(
                        a as i128 - b as i128,
                        <$ty>::MIN as i128,
                        <$ty>::MAX as i128,
                        stringify!($ty),
                    )
                })
            })?;
        };
    }
    macro_rules! export_checked_unsigned_mul {
        ($ty:ty) => {
            engine.export("prim_mul", |_: &State, a: $ty, b: $ty| {
                a.checked_mul(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_signed_mul {
        ($ty:ty) => {
            engine.export("prim_mul", |_: &State, a: $ty, b: $ty| {
                a.checked_mul(b).ok_or_else(|| {
                    checked_integer_error(
                        a as i128 * b as i128,
                        <$ty>::MIN as i128,
                        <$ty>::MAX as i128,
                        stringify!($ty),
                    )
                })
            })?;
        };
    }
    macro_rules! export_checked_int_div {
        ($ty:ty) => {
            engine.export("prim_div", |_: &State, a: $ty, b: $ty| {
                a.checked_div(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_int_rem {
        ($ty:ty) => {
            engine.export("prim_mod", |_: &State, a: $ty, b: $ty| {
                a.checked_rem(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }

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
    export_checked_unsigned_add!(u8);
    export_checked_unsigned_add!(u16);
    export_checked_unsigned_add!(u32);
    export_checked_unsigned_add!(u64);
    export_checked_signed_add!(i8);
    export_checked_signed_add!(i16);
    export_checked_signed_add!(i32);
    export_checked_signed_add!(i64);
    engine.export("prim_add", |_: &State, a: f32, b: f32| Ok(a + b))?;
    engine.export("prim_add", |_: &State, a: f64, b: f64| Ok(a + b))?;
    engine.export("prim_add", |_: &State, a: String, b: String| {
        Ok(format!("{}{}", a, b))
    })?;

    // Subtraction and negation
    export_checked_unsigned_sub!(u8);
    export_checked_unsigned_sub!(u16);
    export_checked_unsigned_sub!(u32);
    export_checked_unsigned_sub!(u64);
    export_checked_signed_sub!(i8);
    export_checked_signed_sub!(i16);
    export_checked_signed_sub!(i32);
    export_checked_signed_sub!(i64);
    engine.export("prim_sub", |_: &State, a: f32, b: f32| Ok(a - b))?;
    engine.export("prim_sub", |_: &State, a: f64, b: f64| Ok(a - b))?;
    engine.export("prim_negate", |_: &State, a: i8| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: i16| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: i32| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: i64| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: f32| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: f64| Ok(-a))?;

    // Multiplication and division
    export_checked_unsigned_mul!(u8);
    export_checked_unsigned_mul!(u16);
    export_checked_unsigned_mul!(u32);
    export_checked_unsigned_mul!(u64);
    export_checked_signed_mul!(i8);
    export_checked_signed_mul!(i16);
    export_checked_signed_mul!(i32);
    export_checked_signed_mul!(i64);
    engine.export("prim_mul", |_: &State, a: f32, b: f32| Ok(a * b))?;
    engine.export("prim_mul", |_: &State, a: f64, b: f64| Ok(a * b))?;
    export_checked_int_div!(u8);
    export_checked_int_div!(u16);
    export_checked_int_div!(u32);
    export_checked_int_div!(u64);
    export_checked_int_div!(i8);
    export_checked_int_div!(i16);
    export_checked_int_div!(i32);
    export_checked_int_div!(i64);
    engine.export("prim_div", |_: &State, a: f32, b: f32| Ok(a / b))?;
    engine.export("prim_div", |_: &State, a: f64, b: f64| Ok(a / b))?;

    // Remainder
    export_checked_int_rem!(u8);
    export_checked_int_rem!(u16);
    export_checked_int_rem!(u32);
    export_checked_int_rem!(u64);
    export_checked_int_rem!(i8);
    export_checked_int_rem!(i16);
    export_checked_int_rem!(i32);
    export_checked_int_rem!(i64);

    // Numeric conversions (used by `std.json`).
    engine.export("prim_to_f64", |_: &State, x: u8| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: u16| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: u32| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: u64| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i8| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i16| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i32| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i64| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: f32| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: f64| Ok(x))?;

    // f64 -> Option <number> conversions (used by `std.json`).
    // - reject NaN/±inf
    // - for integer types: require integral `x` (fract == 0) and in range
    {
        macro_rules! inject_f64_to {
            ($name:literal, $dst_ty:expr, $convert:expr) => {{
                let scheme = Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(Type::builtin(BuiltinTypeId::F64), Type::option($dst_ty)),
                );
                engine.export_native($name, scheme, 1, move |engine, _t, args| {
                    let x = args[0].as_f64()?;
                    let converted: Option<Handle> = $convert(&engine, x)?;
                    option_from_handle(engine.heap(), converted)
                })?;
            }};
        }

        inject_f64_to!(
            "prim_f64_to_u8",
            Type::builtin(BuiltinTypeId::U8),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u8::MIN as f64 && x <= u8::MAX as f64 {
                    Ok(Some(engine.heap().alloc_u8(x as u8)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_u16",
            Type::builtin(BuiltinTypeId::U16),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u16::MIN as f64 && x <= u16::MAX as f64
                {
                    Ok(Some(engine.heap().alloc_u16(x as u16)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_u32",
            Type::builtin(BuiltinTypeId::U32),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u32::MIN as f64 && x <= u32::MAX as f64
                {
                    Ok(Some(engine.heap().alloc_u32(x as u32)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_u64",
            Type::builtin(BuiltinTypeId::U64),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u64::MIN as f64 && x <= u64::MAX as f64
                {
                    Ok(Some(engine.heap().alloc_u64(x as u64)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i8",
            Type::builtin(BuiltinTypeId::I8),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i8::MIN as f64 && x <= i8::MAX as f64 {
                    Ok(Some(engine.heap().alloc_i8(x as i8)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i16",
            Type::builtin(BuiltinTypeId::I16),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i16::MIN as f64 && x <= i16::MAX as f64
                {
                    Ok(Some(engine.heap().alloc_i16(x as i16)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i32",
            Type::builtin(BuiltinTypeId::I32),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i32::MIN as f64 && x <= i32::MAX as f64
                {
                    Ok(Some(engine.heap().alloc_i32(x as i32)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i64",
            Type::builtin(BuiltinTypeId::I64),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i64::MIN as f64 && x <= i64::MAX as f64
                {
                    Ok(Some(engine.heap().alloc_i64(x as i64)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_f32",
            Type::builtin(BuiltinTypeId::F32),
            |engine: &crate::EvaluatorRef<State>, x: f64| -> Result<Option<Handle>, EngineError> {
                if x.is_finite() && x >= f32::MIN as f64 && x <= f32::MAX as f64 {
                    Ok(Some(engine.heap().alloc_f32(x as f32)?))
                } else {
                    Ok(None)
                }
            }
        );
    }

    Ok(())
}

pub(crate) fn inject_json_primops<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    // List/Array conversion helpers.
    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let array_a = Type::array(a);

        let list_to_array_scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), array_a.clone()),
        );
        engine.export_native(
            "prim_array_from_list",
            list_to_array_scheme.clone(),
            1,
            |engine, _, args| {
                let values = args[0].as_list()?;
                engine.heap().alloc_array(values)
            },
        )?;
        engine.export_native("to_array", list_to_array_scheme, 1, |engine, _, args| {
            let values = args[0].as_list()?;
            engine.heap().alloc_array(values)
        })?;

        let array_to_list_scheme = Scheme::new(vec![a_tv], vec![], Type::fun(array_a, list_a));
        engine.export_native(
            "prim_list_from_array",
            array_to_list_scheme.clone(),
            1,
            |engine, _, args| {
                let values = args[0].as_array()?;
                list_from_handles(engine.heap(), values)
            },
        )?;
        engine.export_native("to_list", array_to_list_scheme, 1, |engine, _, args| {
            let values = args[0].as_array()?;
            list_from_handles(engine.heap(), values)
        })?;
    }

    // Dict mapping and traversal helpers (used by `std.json`).
    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_dict_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let dict_ty = arg_tys[1].clone();
            let elem_ty = dict_elem_type(&dict_ty)?;
            let map = engine.heap().pointer_as_dict(&args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::DictMap(
                NativeDictMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    entries: map.into_iter().collect(),
                    next_index: 0,
                    output: BTreeMap::new(),
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
        let e_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("e")));
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
        engine.export_native_scheduler(
            "prim_dict_traverse_result",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let dict_ty = arg_tys[1].clone();
                let elem_ty = dict_elem_type(&dict_ty)?;
                let map = engine.heap().pointer_as_dict(&args[1])?;
                Ok(SchedulerNativeResult::Task(NativeTask::DictTraverseResult(
                    NativeDictTraverseResult {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        entries: map.into_iter().collect(),
                        next_index: 0,
                        output: BTreeMap::new(),
                    },
                )))
            },
        )?;
    }

    // Parsing helpers used by `std.json` instances.
    {
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let uuid_ty = Type::builtin(BuiltinTypeId::Uuid);
        let scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(string_ty.clone(), Type::option(uuid_ty)),
        );
        engine.export_native("prim_parse_uuid", scheme, 1, |engine, _, args| {
            let s = args[0].as_string()?;
            let parsed = Uuid::parse_str(&s)
                .ok()
                .map(|uuid| engine.heap().alloc_uuid(uuid))
                .transpose()?;
            option_from_handle(engine.heap(), parsed)
        })?;
    }

    {
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let dt_ty = Type::builtin(BuiltinTypeId::DateTime);
        let scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(string_ty.clone(), Type::option(dt_ty)),
        );
        engine.export_native("prim_parse_datetime", scheme, 1, |engine, _, args| {
            let s = args[0].as_string()?;
            let parsed = DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .map(|dt| engine.heap().alloc_datetime(dt))
                .transpose()?;
            option_from_handle(engine.heap(), parsed)
        })?;
    }

    // prim_json_stringify : a -> string
    //
    // Used by `std.json` to implement `Show Value` (JSON-encoded string).
    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let string_ty = Type::builtin(BuiltinTypeId::String);
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
            null: Symbol::intern("Null"),
            bool_: Symbol::intern("Bool"),
            string: Symbol::intern("String"),
            number: Symbol::intern("Number"),
            array: Symbol::intern("Array"),
            object: Symbol::intern("Object"),
        };

        fn to_serde_json(
            heap: &HeapAccess<'_>,
            v: &Cell,
            tags: &Tags,
        ) -> Option<serde_json::Value> {
            match v {
                Cell::Adt(tag, _) if tag == &tags.null => Some(serde_json::Value::Null),
                Cell::Adt(tag, args) if tag == &tags.bool_ => match args.as_slice() {
                    [arg] => heap
                        .get(arg)
                        .ok()?
                        .cell_as_bool()
                        .ok()
                        .map(serde_json::Value::Bool),
                    _ => None,
                },
                Cell::Adt(tag, args) if tag == &tags.string => match args.as_slice() {
                    [arg] => heap
                        .get(arg)
                        .ok()?
                        .cell_as_string()
                        .ok()
                        .map(serde_json::Value::String),
                    _ => None,
                },
                Cell::Adt(tag, args) if tag == &tags.number => match args.as_slice() {
                    [arg] => {
                        let n = heap.get(arg).ok()?.cell_as_f64().ok()?;
                        serde_json::Number::from_f64(n)
                            .map(serde_json::Value::Number)
                            .or(Some(serde_json::Value::Null))
                    }
                    _ => None,
                },
                Cell::Adt(tag, args) if tag == &tags.array => match args.as_slice() {
                    [arg] => {
                        let xs = heap.get(arg).ok()?.cell_as_array().ok()?;
                        let mut out = Vec::with_capacity(xs.len());
                        for x in &xs {
                            let x_value = heap.get(x).ok()?;
                            out.push(to_serde_json(heap, x_value, tags)?);
                        }
                        Some(serde_json::Value::Array(out))
                    }
                    _ => None,
                },
                Cell::Adt(tag, args) if tag == &tags.object => match args.as_slice() {
                    [arg] => {
                        let map = heap.get(arg).ok()?.cell_as_dict().ok()?;
                        let mut out = serde_json::Map::with_capacity(map.len());
                        for (k, v) in &map {
                            let v_value = heap.get(v).ok()?;
                            out.insert(k.as_ref().to_string(), to_serde_json(heap, v_value, tags)?);
                        }
                        Some(serde_json::Value::Object(out))
                    }
                    _ => None,
                },
                _ => None,
            }
        }

        engine.export_native("prim_json_stringify", scheme, 1, move |engine, _, args| {
            let pointer = args[0].pointer()?;
            let json = engine.heap().with_access(|heap| {
                let value = heap.get(&pointer)?;
                Ok(to_serde_json(heap, value, &tags))
            })?;
            let Some(json) = json else {
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
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let result_con = Type::builtin(BuiltinTypeId::Result);
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
            null: Symbol::intern("Null"),
            bool_: Symbol::intern("Bool"),
            string: Symbol::intern("String"),
            number: Symbol::intern("Number"),
            array: Symbol::intern("Array"),
            object: Symbol::intern("Object"),
        };

        fn to_json_value(
            v: &serde_json::Value,
            tags: &Tags,
            heap: &Heap,
        ) -> Result<Handle, EngineError> {
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
                        let value = to_json_value(x, tags, heap)?;
                        out.push(value);
                    }
                    let array = heap.alloc_array(out)?;
                    heap.alloc_adt(tags.array.clone(), vec![array])
                }
                serde_json::Value::Object(obj) => {
                    let mut out = BTreeMap::new();
                    for (k, v) in obj {
                        let value = to_json_value(v, tags, heap)?;
                        out.insert(Symbol::intern(k.as_str()), value);
                    }
                    let dict = heap.alloc_dict(out)?;
                    heap.alloc_adt(tags.object.clone(), vec![dict])
                }
            }
        }

        fn result_ok(heap: &Heap, v: Handle) -> Result<Handle, EngineError> {
            heap.alloc_adt(Symbol::intern("Ok"), vec![v])
        }

        fn result_err(heap: &Heap, msg: String) -> Result<Handle, EngineError> {
            let msg = heap.alloc_string(msg)?;
            heap.alloc_adt(Symbol::intern("Err"), vec![msg])
        }

        engine.export_native("prim_json_parse", scheme, 1, move |engine, _, args| {
            let s = args[0].as_string()?;
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

pub(crate) fn inject_list_builtins<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = expect_list(engine.heap(), &args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceMap(
                NativeSequenceMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::List,
                    next_index: 0,
                    output: Vec::new(),
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty)?;
            let values = expect_array(engine.heap(), &args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceMap(
                NativeSequenceMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::Array,
                    next_index: 0,
                    output: Vec::new(),
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(a, array_a));
        engine.export_native("prim_array_singleton", scheme, 1, |engine, _, args| {
            engine.heap().alloc_array(vec![args[0].clone()])
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            match option_value(engine.heap(), &args[1])? {
                Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryMap(
                    NativeUnaryMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        value,
                        shape: NativeUnaryShape::Option,
                    },
                ))),
                None => Ok(SchedulerNativeResult::Ready(option_from_pointer(
                    engine.heap(),
                    None,
                )?)),
            }
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
        let e_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("e")));
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
        engine.export_native_scheduler("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            let (ok_ty, _err_ty) = result_types(&result_ty)?;
            match result_value(engine.heap(), &args[1])? {
                Ok(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryMap(
                    NativeUnaryMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: ok_ty,
                        value,
                        shape: NativeUnaryShape::Result,
                    },
                ))),
                Err(err) => Ok(SchedulerNativeResult::Ready(result_from_pointer(
                    engine.heap(),
                    Err(err),
                )?)),
            }
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_foldl", scheme, 3, |engine, call_type, args| {
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
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_foldl", scheme, 3, |engine, call_type, args| {
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
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_foldl", scheme, 3, |engine, call_type, args| {
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
            let values = option_value(engine.heap(), &args[2])?.into_iter().collect();
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_foldr", scheme, 3, |engine, call_type, args| {
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
            let mut values = expect_list(engine.heap(), &args[2])?;
            values.reverse();
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Right,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_foldr", scheme, 3, |engine, call_type, args| {
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
            let mut values = expect_array(engine.heap(), &args[2])?;
            values.reverse();
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Right,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_foldr", scheme, 3, |engine, call_type, args| {
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
            let values = option_value(engine.heap(), &args[2])?.into_iter().collect();
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Right,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_fold", scheme, 3, |engine, call_type, args| {
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
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_fold", scheme, 3, |engine, call_type, args| {
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
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_fold", scheme, 3, |engine, call_type, args| {
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
            let values = option_value(engine.heap(), &args[2])?.into_iter().collect();
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::builtin(BuiltinTypeId::Bool)),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.export_native_scheduler("prim_filter", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = expect_list(engine.heap(), &args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceFilter(
                NativeSequenceFilter {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::List,
                    next_index: 0,
                    output: Vec::new(),
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::builtin(BuiltinTypeId::Bool)),
                Type::fun(array_a.clone(), array_a),
            ),
        );
        engine.export_native_scheduler("prim_filter", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty)?;
            let values = expect_array(engine.heap(), &args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceFilter(
                NativeSequenceFilter {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::Array,
                    next_index: 0,
                    output: Vec::new(),
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::builtin(BuiltinTypeId::Bool)),
                Type::fun(opt_a.clone(), opt_a),
            ),
        );
        engine.export_native_scheduler("prim_filter", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            match option_value(engine.heap(), &args[1])? {
                Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFilter(
                    NativeUnaryFilter {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        value,
                        original: args[1],
                    },
                ))),
                None => Ok(SchedulerNativeResult::Ready(option_from_pointer(
                    engine.heap(),
                    None,
                )?)),
            }
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler(
            "prim_filter_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let values = expect_list(engine.heap(), &args[1])?;
                Ok(SchedulerNativeResult::Task(NativeTask::SequenceFilterMap(
                    NativeSequenceFilterMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        values,
                        shape: NativeSequenceShape::List,
                        next_index: 0,
                        output: Vec::new(),
                    },
                )))
            },
        )?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler(
            "prim_filter_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let array_ty = arg_tys[1].clone();
                let elem_ty = array_elem_type(&array_ty)?;
                let values = expect_array(engine.heap(), &args[1])?;
                Ok(SchedulerNativeResult::Task(NativeTask::SequenceFilterMap(
                    NativeSequenceFilterMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        values,
                        shape: NativeSequenceShape::Array,
                        next_index: 0,
                        output: Vec::new(),
                    },
                )))
            },
        )?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler(
            "prim_filter_map",
            scheme,
            2,
            |engine, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                match option_value(engine.heap(), &args[1])? {
                    Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFilterMap(
                        NativeUnaryFilterMap {
                            func: args[0],
                            func_type: func_ty,
                            elem_type: elem_ty,
                            value,
                        },
                    ))),
                    None => Ok(SchedulerNativeResult::Ready(option_from_pointer(
                        engine.heap(),
                        None,
                    )?)),
                }
            },
        )?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = expect_list(engine.heap(), &args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceFlatMap(
                NativeSequenceFlatMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::List,
                    next_index: 0,
                    output: Vec::new(),
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty)?;
            let values = expect_array(engine.heap(), &args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceFlatMap(
                NativeSequenceFlatMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::Array,
                    next_index: 0,
                    output: Vec::new(),
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
        engine.export_native_scheduler("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            match option_value(engine.heap(), &args[1])? {
                Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFlatMap(
                    NativeUnaryFlatMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        value,
                        shape: NativeUnaryShape::Option,
                    },
                ))),
                None => Ok(SchedulerNativeResult::Ready(option_from_pointer(
                    engine.heap(),
                    None,
                )?)),
            }
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
        let e_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("e")));
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
        engine.export_native_scheduler("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            let (ok_ty, _err_ty) = result_types(&result_ty)?;
            match result_value(engine.heap(), &args[1])? {
                Ok(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFlatMap(
                    NativeUnaryFlatMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: ok_ty,
                        value,
                        shape: NativeUnaryShape::Result,
                    },
                ))),
                Err(err) => Ok(SchedulerNativeResult::Ready(result_from_pointer(
                    engine.heap(),
                    Err(err),
                )?)),
            }
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
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
        engine.export_native_scheduler("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            if !expect_list(engine.heap(), &args[1])?.is_empty() {
                return Ok(SchedulerNativeResult::Ready(args[1]));
            }
            Ok(SchedulerNativeResult::Task(NativeTask::ApplyUnary(
                NativeApplyUnary {
                    func: args[0],
                    func_type: func_ty,
                    arg: args[1],
                    arg_type: list_ty,
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
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
        engine.export_native_scheduler("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            if !expect_array(engine.heap(), &args[1])?.is_empty() {
                return Ok(SchedulerNativeResult::Ready(args[1]));
            }
            Ok(SchedulerNativeResult::Task(NativeTask::ApplyUnary(
                NativeApplyUnary {
                    func: args[0],
                    func_type: func_ty,
                    arg: args[1],
                    arg_type: array_ty,
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
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
        engine.export_native_scheduler("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            if option_value(engine.heap(), &args[1])?.is_some() {
                return Ok(SchedulerNativeResult::Ready(args[1]));
            }
            Ok(SchedulerNativeResult::Task(NativeTask::ApplyUnary(
                NativeApplyUnary {
                    func: args[0],
                    func_type: func_ty,
                    arg: args[1],
                    arg_type: opt_ty,
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let e_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("e")));
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
        engine.export_native_scheduler("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            if result_value(engine.heap(), &args[1])?.is_ok() {
                return Ok(SchedulerNativeResult::Ready(args[1]));
            }
            Ok(SchedulerNativeResult::Task(NativeTask::ApplyUnary(
                NativeApplyUnary {
                    func: args[0],
                    func_type: func_ty,
                    arg: args[1],
                    arg_type: result_ty,
                },
            )))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.export_native_scheduler("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = expect_list(engine.heap(), &args[0])?;
            Ok(SchedulerNativeResult::Task(NativeTask::Sum(NativeSum {
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.export_native_scheduler("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty)?;
            let values = expect_array(engine.heap(), &args[0])?;
            Ok(SchedulerNativeResult::Task(NativeTask::Sum(NativeSum {
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.export_native_scheduler("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            let values = option_value(engine.heap(), &args[0])?.into_iter().collect();
            Ok(SchedulerNativeResult::Task(NativeTask::Sum(NativeSum {
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.export_native_scheduler("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = expect_list(engine.heap(), &args[0])?;
            if values.is_empty() {
                return Err(EngineError::EmptySequence);
            }
            Ok(SchedulerNativeResult::Task(NativeTask::Mean(NativeMean {
                len: values.len(),
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeMeanState::Enter,
                next_index: 0,
                step: None,
                len_value: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.export_native_scheduler("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty)?;
            let values = expect_array(engine.heap(), &args[0])?;
            if values.is_empty() {
                return Err(EngineError::EmptySequence);
            }
            Ok(SchedulerNativeResult::Task(NativeTask::Mean(NativeMean {
                len: values.len(),
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeMeanState::Enter,
                next_index: 0,
                step: None,
                len_value: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.export_native_scheduler("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            let values = match option_value(engine.heap(), &args[0])? {
                Some(value) => vec![value],
                None => return Err(EngineError::EmptySequence),
            };
            Ok(SchedulerNativeResult::Task(NativeTask::Mean(NativeMean {
                len: 1,
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeMeanState::Enter,
                next_index: 0,
                step: None,
                len_value: None,
            })))
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), Type::builtin(BuiltinTypeId::I32)),
        );
        engine.export_native("count", scheme, 1, |engine, _, args| {
            engine.heap().alloc_i32(args[0].as_list()?.len() as i32)
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), Type::builtin(BuiltinTypeId::I32)),
        );
        engine.export_native("count", scheme, 1, |engine, _, args| {
            engine.heap().alloc_i32(args[0].as_array()?.len() as i32)
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), Type::builtin(BuiltinTypeId::I32)),
        );
        engine.export_native("count", scheme, 1, |engine, _, args| {
            engine
                .heap()
                .alloc_i32(option_handle(&args[0])?.is_some() as i32)
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.export_native("prim_take", scheme, 2, |engine, _, args| {
            let n = args[0].as_i32()?;
            let n = as_nonneg_usize(n);
            let xs = args[1].as_list()?;
            list_from_handles(engine.heap(), xs.into_iter().take(n).collect())
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(array_a.clone(), array_a),
            ),
        );
        engine.export_native("prim_take", scheme, 2, |engine, _, args| {
            let n = args[0].as_i32()?;
            let n = as_nonneg_usize(n);
            let xs = args[1].as_array()?;
            engine.heap().alloc_array(xs.into_iter().take(n).collect())
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.export_native("prim_skip", scheme, 2, |engine, _, args| {
            let n = args[0].as_i32()?;
            let n = as_nonneg_usize(n);
            let xs = args[1].as_list()?;
            list_from_handles(engine.heap(), xs.into_iter().skip(n).collect())
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(array_a.clone(), array_a),
            ),
        );
        engine.export_native("prim_skip", scheme, 2, |engine, _, args| {
            let n = args[0].as_i32()?;
            let n = as_nonneg_usize(n);
            let xs = args[1].as_array()?;
            engine.heap().alloc_array(xs.into_iter().skip(n).collect())
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::list(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(list_a.clone(), a.clone()),
            ),
        );
        engine.export_native("prim_get", scheme, 2, |_, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let list_ty = arg_tys[1].clone();
            let _elem_ty = list_elem_type(&list_ty)?;
            let idx = args[0].as_i32()?;
            let xs = args[1].as_list()?;
            let idx = checked_index(Symbol::intern("prim_get"), idx, xs.len())?;
            Ok(xs[idx].clone())
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::array(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(array_a.clone(), a.clone()),
            ),
        );
        engine.export_native("prim_get", scheme, 2, |_, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let array_ty = arg_tys[1].clone();
            let _elem_ty = array_elem_type(&array_ty)?;
            let idx = args[0].as_i32()?;
            let xs = args[1].as_array()?;
            let idx = checked_index(Symbol::intern("prim_get"), idx, xs.len())?;
            Ok(xs[idx].clone())
        })?;
    }

    for size in 2..=32 {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let tuple = Type::tuple(vec![a.clone(); size]);
        let scheme = Scheme::new(
            vec![a_tv],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(tuple.clone(), a.clone()),
            ),
        );
        engine.export_native("prim_get", scheme, 2, move |_, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let tuple_ty = arg_tys[1].clone();
            let _elem_ty = tuple_elem_type(&tuple_ty)?;
            let idx = args[0].as_i32()?;
            let idx_usize = checked_index(Symbol::intern("prim_get"), idx, size)?;
            let xs = args[1].as_tuple()?;
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
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
            let xs = args[0].as_list()?;
            let ys = args[1].as_list()?;
            let zipped = zip_tuple2_handles(engine.heap(), xs, ys)?;
            list_from_handles(engine.heap(), zipped)
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
            let xs = args[0].as_array()?;
            let ys = args[1].as_array()?;
            let zipped = zip_tuple2_handles(engine.heap(), xs, ys)?;
            engine.heap().alloc_array(zipped)
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
            let pairs = args[0].as_list()?;
            let (left, right) = unzip_tuple2_handles(pairs)?;
            let left = list_from_handles(engine.heap(), left)?;
            let right = list_from_handles(engine.heap(), right)?;
            engine.heap().alloc_tuple(vec![left, right])
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let b_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("b")));
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
            let pairs = args[0].as_array()?;
            let (left, right) = unzip_tuple2_handles(pairs)?;
            let left = engine.heap().alloc_array(left)?;
            let right = engine.heap().alloc_array(right)?;
            engine.heap().alloc_tuple(vec![left, right])
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
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
            let values = args[0].as_list()?;
            extremum_handle_by_type(
                engine.heap(),
                "min",
                &elem_ty,
                values,
                std::cmp::Ordering::Less,
            )
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
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
            let values = args[0].as_array()?;
            extremum_handle_by_type(
                engine.heap(),
                "min",
                &elem_ty,
                values,
                std::cmp::Ordering::Less,
            )
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.export_native("min", scheme, 1, |_, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty)?;
            match option_handle(&args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
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
            let values = args[0].as_list()?;
            extremum_handle_by_type(
                engine.heap(),
                "max",
                &elem_ty,
                values,
                std::cmp::Ordering::Greater,
            )
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
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
            let values = args[0].as_array()?;
            extremum_handle_by_type(
                engine.heap(),
                "max",
                &elem_ty,
                values,
                std::cmp::Ordering::Greater,
            )
        })?;
    }

    {
        let a_tv = engine.type_system.fresh_type_var(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let opt_a = Type::option(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.export_native("max", scheme, 1, |_, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty)?;
            match option_handle(&args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    Ok(())
}

pub(crate) fn inject_option_result_builtins<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
) -> Result<(), EngineError> {
    let unwrap = Symbol::intern("unwrap");
    let unwrap_schemes = engine
        .type_system
        .env
        .lookup(&unwrap)
        .ok_or_else(|| EngineError::UnknownVar(unwrap.clone()))?
        .to_vec();
    for scheme in unwrap_schemes {
        let typ = scheme.typ.clone();
        match typ.as_ref() {
            TypeKind::Fun(arg_ty, _)
                if matches!(
                    arg_ty.as_ref(),
                    TypeKind::App(head, _)
                        if matches!(
                            head.as_ref(),
                            TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Option)
                        )
                ) =>
            {
                engine.export_native("unwrap", scheme, 1, |_, _, args| {
                    match option_handle(&args[0])? {
                        Some(value) => Ok(value),
                        None => Err(EngineError::Custom("called unwrap on None".into())),
                    }
                })?;
            }
            TypeKind::Fun(arg_ty, _)
                if matches!(
                    arg_ty.as_ref(),
                    TypeKind::App(head, _)
                        if matches!(
                            head.as_ref(),
                            TypeKind::App(head2, _)
                                if matches!(
                                    head2.as_ref(),
                                    TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Result)
                                )
                        )
                ) =>
            {
                engine.export_native("unwrap", scheme, 1, |_, _, args| {
                    match result_handle(&args[0])? {
                        Ok(value) => Ok(value),
                        Err(_) => Err(EngineError::Custom("called unwrap on Err".into())),
                    }
                })?;
            }
            _ => {}
        }
    }

    let is_some = Symbol::intern("is_some");
    let is_some_scheme = engine.lookup_scheme(&is_some)?;
    engine.export_native("is_some", is_some_scheme, 1, |engine, _, args| {
        engine.heap().alloc_bool(option_handle(&args[0])?.is_some())
    })?;
    let is_none = Symbol::intern("is_none");
    let is_none_scheme = engine.lookup_scheme(&is_none)?;
    engine.export_native("is_none", is_none_scheme, 1, |engine, _, args| {
        engine.heap().alloc_bool(option_handle(&args[0])?.is_none())
    })?;

    let is_ok = Symbol::intern("is_ok");
    let is_ok_scheme = engine.lookup_scheme(&is_ok)?;
    engine.export_native("is_ok", is_ok_scheme, 1, |engine, _, args| {
        engine.heap().alloc_bool(result_handle(&args[0])?.is_ok())
    })?;
    let is_err = Symbol::intern("is_err");
    let is_err_scheme = engine.lookup_scheme(&is_err)?;
    engine.export_native("is_err", is_err_scheme, 1, |engine, _, args| {
        engine.heap().alloc_bool(result_handle(&args[0])?.is_err())
    })?;
    Ok(())
}
