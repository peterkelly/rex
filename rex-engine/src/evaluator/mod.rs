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
    compiler::program::CompiledProgram,
    error::EngineError,
    evaluator::{context::InternalCtx, eval::eval_typed_expr, runtime_core::RuntimeCore},
    memory::heap::{Handle, Heap, RootScope, RootedPtr},
    stack::FrameId,
    util::split_fun,
};

pub(crate) mod context;
pub(crate) mod eval;
pub(crate) mod host_action;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub(crate) struct CallSite {
    pub parent: Option<FrameId>,
}

impl CallSite {
    pub(crate) fn child(parent: FrameId) -> Self {
        Self {
            parent: Some(parent),
        }
    }
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

    /// Heap used by this evaluator runtime.
    pub fn heap(&self) -> &Heap {
        &self.heap
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
        self.run_with_context(program, inputs)
            .await
            .map(|(value, _ctx)| value)
    }

    /// Run one prepared program and keep a host-call context for follow-up host work.
    ///
    /// This is used by embedders that treat the evaluated result as a host-managed action and
    /// need to resume Rex callbacks after the top-level expression has produced that action.
    pub async fn run_with_context(
        self,
        program: CompiledProgram,
        inputs: BTreeMap<String, Handle>,
    ) -> Result<(Handle, InternalCtx<State>), EngineError> {
        let runtime = self.runtime;
        let main_signature = program.main_signature().clone();
        let args = main_input_args(&self.heap, &main_signature, &inputs)?;
        let value = eval_typed_expr(
            runtime.clone(),
            &self.heap,
            program.env,
            Arc::clone(&program.expr),
            args,
        )
        .await?;
        let ctx = InternalCtx::new_at_call_site(&runtime, CallSite { parent: None });
        Ok((value, ctx))
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
            handle.ensure_heap(heap)?;
            Ok((handle.clone(), input.typ.clone()))
        })
        .collect()
}

pub(crate) fn resolve_arg_type<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    arg_type: Option<&Type>,
    arg: RootedPtr<'scope>,
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
