use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use rex_ast::Symbol;
use rex_typesystem::{
    types::{BuiltinTypeId, Type, Types},
    typesystem::TypeSystem,
    unification::unify,
};

use crate::{
    compiler::program::CompiledProgram,
    error::EngineError,
    evaluator::{eval::eval_typed_expr, runtime_core::RuntimeCore},
    util::split_fun,
    value::{Cell, Handle, Heap, HeapAccess, Pointer},
};

pub(crate) mod context;
pub(crate) mod eval;
pub(crate) mod native_callable;
pub(crate) mod native_functions;
pub(crate) mod runtime_core;
pub(crate) mod scheduler;

/// Single-shot runtime for running prepared Rex code.
///
/// `run` consumes both the evaluator and the [`CompiledProgram`], and applies
/// the supplied runtime inputs to the program's external `main` interface.
pub struct Evaluator<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) runtime: RuntimeCore<State>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub(crate) struct CallSite {
    pub parent: Option<Pointer>,
}

impl CallSite {
    pub(crate) fn child(parent: Pointer) -> Self {
        Self {
            parent: Some(parent),
        }
    }
}

impl<State> Evaluator<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(runtime: RuntimeCore<State>) -> Self {
        Self { runtime }
    }

    /// Type system captured by the evaluator runtime.
    pub fn type_system(&self) -> Arc<TypeSystem> {
        Arc::clone(&self.runtime.type_system)
    }

    /// Heap used by this evaluator runtime.
    pub fn heap(&self) -> &Heap {
        &self.runtime.heap
    }

    /// Run one prepared program with runtime main inputs.
    ///
    /// The `inputs` map must contain one [`Handle`] for each parameter in the
    /// program's main signature, keyed by parameter name. When using the
    /// top-level `rex` crate and those inputs are available as JSON, callers
    /// can build this map with `rex::json::json_to_main_inputs`.
    pub async fn run(
        self,
        program: CompiledProgram,
        inputs: BTreeMap<String, Handle>,
    ) -> Result<Handle, EngineError> {
        let runtime = self.runtime;
        let main_signature = program.main_signature().clone();
        let args = main_input_args(&runtime.heap, &main_signature, &inputs)?;
        eval_typed_expr(runtime, program.env, Arc::clone(&program.expr), args).await
    }
}

fn main_input_args(
    heap: &Heap,
    signature: &crate::MainSignature,
    inputs: &BTreeMap<String, Handle>,
) -> Result<Vec<(Handle, Type)>, EngineError> {
    let expected = signature
        .inputs()
        .iter()
        .map(|input| input.name.clone())
        .collect::<BTreeSet<_>>();
    let actual = inputs.keys().cloned().collect::<BTreeSet<_>>();
    if expected != actual {
        let missing = expected.difference(&actual).cloned().collect();
        let extra = actual.difference(&expected).cloned().collect();
        return Err(EngineError::MainInputMismatch { missing, extra });
    }

    signature
        .inputs()
        .iter()
        .map(|input| {
            let handle = inputs.get(&input.name).ok_or_else(|| {
                EngineError::Internal("validated input map was incomplete".into())
            })?;
            handle.pointer_for_heap(heap)?;
            Ok((handle.clone(), input.typ.clone()))
        })
        .collect()
}

fn cell_type(heap: &HeapAccess<'_>, cell: &Cell) -> Result<Type, EngineError> {
    let pointer_type = |pointer: &Pointer| -> Result<Type, EngineError> {
        let cell = heap.get(pointer)?;
        cell_type(heap, cell)
    };

    match cell {
        Cell::Bool(..) => Ok(Type::builtin(BuiltinTypeId::Bool)),
        Cell::U8(..) => Ok(Type::builtin(BuiltinTypeId::U8)),
        Cell::U16(..) => Ok(Type::builtin(BuiltinTypeId::U16)),
        Cell::U32(..) => Ok(Type::builtin(BuiltinTypeId::U32)),
        Cell::U64(..) => Ok(Type::builtin(BuiltinTypeId::U64)),
        Cell::I8(..) => Ok(Type::builtin(BuiltinTypeId::I8)),
        Cell::I16(..) => Ok(Type::builtin(BuiltinTypeId::I16)),
        Cell::I32(..) => Ok(Type::builtin(BuiltinTypeId::I32)),
        Cell::I64(..) => Ok(Type::builtin(BuiltinTypeId::I64)),
        Cell::F32(..) => Ok(Type::builtin(BuiltinTypeId::F32)),
        Cell::F64(..) => Ok(Type::builtin(BuiltinTypeId::F64)),
        Cell::String(..) => Ok(Type::builtin(BuiltinTypeId::String)),
        Cell::Uuid(..) => Ok(Type::builtin(BuiltinTypeId::Uuid)),
        Cell::DateTime(..) => Ok(Type::builtin(BuiltinTypeId::DateTime)),
        Cell::Tuple(elems) => {
            let mut tys = Vec::with_capacity(elems.len());
            for elem in elems {
                tys.push(pointer_type(elem)?);
            }
            Ok(Type::tuple(tys))
        }
        Cell::Array(elems) => {
            let first = elems
                .first()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("array")))?;
            let elem_ty = pointer_type(first)?;
            for elem in elems.iter().skip(1) {
                let ty = pointer_type(elem)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::Array), elem_ty))
        }
        Cell::Dict(map) => {
            let first = map
                .values()
                .next()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("dict")))?;
            let elem_ty = pointer_type(first)?;
            for val in map.values().skip(1) {
                let ty = pointer_type(val)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::Dict), elem_ty))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Some" && args.len() == 1 => {
            let inner = pointer_type(&args[0])?;
            Ok(Type::app(Type::builtin(BuiltinTypeId::Option), inner))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "None" && args.is_empty() => {
            Err(EngineError::UnknownType(Symbol::intern("option")))
        }
        Cell::Adt(tag, args)
            if (tag.as_ref() == "Ok" || tag.as_ref() == "Err") && args.len() == 1 =>
        {
            Err(EngineError::UnknownType(Symbol::intern("result")))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Empty" && args.is_empty() => {
            Err(EngineError::UnknownType(Symbol::intern("list")))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
            let elem_ty = pointer_type(&args[0])?;
            Ok(Type::app(Type::builtin(BuiltinTypeId::List), elem_ty))
        }
        Cell::Adt(tag, _args) if tag.as_ref() == "Empty" || tag.as_ref() == "Cons" => {
            Err(EngineError::NativeType {
                expected: "list".into(),
                got: cell.cell_type_name().into(),
            })
        }
        Cell::Adt(tag, _args) => Err(EngineError::UnknownType(tag.clone())),
        Cell::Uninitialized(..) => Err(EngineError::UnknownType(Symbol::intern("uninitialized"))),
        Cell::Frame(..) => Err(EngineError::UnknownType(Symbol::intern("frame"))),
        Cell::Closure(..) => Err(EngineError::UnknownType(Symbol::intern("closure"))),
        Cell::Native(..) => Err(EngineError::UnknownType(Symbol::intern("native"))),
        Cell::Overloaded(..) => Err(EngineError::UnknownType(Symbol::intern("overloaded"))),
    }
}

pub(crate) fn resolve_arg_type(
    heap: &Heap,
    arg_type: Option<&Type>,
    arg: &Pointer,
) -> Result<Type, EngineError> {
    let infer_from_cell = |ty_hint: Option<&Type>| -> Result<Type, EngineError> {
        heap.with_access(|heap| {
            let cell = heap.get(arg)?;
            match ty_hint {
                Some(ty) => match cell_type(heap, cell) {
                    Ok(val_ty) if val_ty.ftv().is_empty() => Ok(val_ty),
                    _ => Ok(ty.clone()),
                },
                None => cell_type(heap, cell),
            }
        })
    };
    match arg_type {
        Some(ty) if ty.ftv().is_empty() => Ok(ty.clone()),
        Some(ty) => infer_from_cell(Some(ty)),
        None => infer_from_cell(None),
    }
}

pub(crate) fn application_result_type(
    func_type: &Type,
    arg_type: &Type,
) -> Result<Type, EngineError> {
    let (expected_arg, result) =
        split_fun(func_type).ok_or_else(|| EngineError::NotCallable(func_type.to_string()))?;
    let subst = unify(&expected_arg, arg_type).map_err(|_| EngineError::NativeType {
        expected: expected_arg.to_string(),
        got: arg_type.to_string(),
    })?;
    Ok(result.apply(&subst))
}
