use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use rex_typesystem::{
    types::{Type, Types},
    typesystem::TypeSystem,
    unification::unify,
};

use crate::{
    Value,
    compiler::program::CompiledProgram,
    error::EngineError,
    evaluator::{eval::eval_typed_expr, runtime_core::RuntimeCore},
    memory::heap::{Heap, RootScope, RootedPtr},
    util::split_fun,
};

pub(crate) mod context;
pub(crate) mod eval;
pub(crate) mod intrinsic_handler;
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
    pub(crate) heap: Heap,
}

impl<State> Evaluator<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(runtime: RuntimeCore<State>, heap: Heap) -> Self {
        Self { runtime, heap }
    }

    /// Type system captured by the evaluator runtime.
    pub fn type_system(&self) -> Arc<TypeSystem> {
        Arc::clone(&self.runtime.type_system)
    }

    /// Run one prepared program with runtime main inputs.
    ///
    /// The `inputs` map must contain one owned [`Value`] for each parameter in
    /// the program's main signature, keyed by parameter name.
    pub async fn run(
        self,
        program: CompiledProgram,
        inputs: BTreeMap<String, Value>,
    ) -> Result<Value, EngineError> {
        let Self { runtime, mut heap } = self;
        let main_signature = program.main_signature().clone();
        let mut result_type = program.result_type().clone();
        let args = main_input_args(&main_signature, inputs)?;
        let result = eval_typed_expr(
            runtime.clone(),
            &mut heap,
            program.env,
            Arc::clone(&program.expr),
            args,
        )
        .await?;
        if !result_type.ftv().is_empty() {
            let actual_type = heap.machine_root_scope(|scope| scope.infer_type(result))?;
            let subst = unify(&result_type, &actual_type).map_err(|_| EngineError::NativeType {
                expected: result_type.to_string(),
                got: actual_type.to_string(),
            })?;
            result_type = result_type.apply(&subst);
        }
        heap.machine_root_scope(|scope| {
            scope.export_value(result, &result_type, runtime.type_system.as_ref())
        })
    }
}

fn main_input_args(
    signature: &crate::MainSignature,
    mut inputs: BTreeMap<String, Value>,
) -> Result<Vec<(Value, Type)>, EngineError> {
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
            let value = inputs.remove(&input.name).ok_or_else(|| {
                EngineError::Internal("validated input map was incomplete".into())
            })?;
            Ok((value, input.typ.clone()))
        })
        .collect()
}

pub(crate) fn resolve_arg_type(
    scope: &mut RootScope<'_>,
    arg_type: Option<&Type>,
    arg: RootedPtr,
) -> Result<Type, EngineError> {
    let infer_from_value = |ty_hint: Option<&Type>| -> Result<Type, EngineError> {
        match ty_hint {
            Some(ty) => match scope.infer_type(arg) {
                Ok(val_ty) if val_ty.ftv().is_empty() => Ok(val_ty),
                _ => Ok(ty.clone()),
            },
            None => scope.infer_type(arg),
        }
    };
    match arg_type {
        Some(ty) if ty.ftv().is_empty() => Ok(ty.clone()),
        Some(ty) => infer_from_value(Some(ty)),
        None => infer_from_value(None),
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
