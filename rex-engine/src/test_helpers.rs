use rex_typesystem::types::{Scheme, Type};
use rex_util::GasMeter;

use crate::engine::{
    EvalStop, RuntimeSnapshot, check_runtime_cancelled, eval_typed_expr,
    eval_typed_expr_from_parent, resolve_arg_type, synthetic_application_expr,
};
use crate::evaluator::EvalContext;
use crate::{Closure, Engine, EngineError, Environment, EvaluatorRef, Export, NativeFuture};
use crate::{Pointer, Value};

impl<State> Engine<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn export_native_async<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(EvaluatorRef<State>, Type, Vec<Pointer>) -> NativeFuture + Send + Sync + 'static,
    {
        self.export_native_async_with_gas_cost(name, scheme, arity, 0, handler)
    }

    pub(crate) fn export_native_async_with_gas_cost<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(EvaluatorRef<State>, Type, Vec<Pointer>) -> NativeFuture + Send + Sync + 'static,
    {
        let export =
            Export::from_native_async_with_gas_cost(name, scheme, arity, gas_cost, handler)?;
        self.inject_root_export(export)
    }
}

impl<State> EvaluatorRef<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn parent_frame(&self) -> Option<Pointer> {
        self.context.parent
    }

    pub(crate) async fn apply_pointer(
        &self,
        func: Pointer,
        arg: Pointer,
        func_type: Option<&Type>,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let runtime: &RuntimeSnapshot<State> = self;
        apply_with_context(runtime, func, arg, func_type, arg_type, gas, self.context).await
    }
}

pub(crate) async fn eval_typed_expr_child<State>(
    runtime: &RuntimeSnapshot<State>,
    parent: Pointer,
    env: &Environment,
    expr: &rex_typesystem::types::TypedExpr,
    gas: &mut GasMeter,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    check_runtime_cancelled(runtime)?;
    eval_typed_expr_from_parent(runtime, parent, EvalStop::Parent(parent), env, expr, gas).await
}

pub(crate) async fn apply_with_context<State>(
    runtime: &RuntimeSnapshot<State>,
    func: Pointer,
    arg: Pointer,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
    gas: &mut GasMeter,
    context: EvalContext,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let func_type = match func_type {
        Some(typ) => typ.clone(),
        None => callable_pointer_type(runtime, &func)?,
    };
    let arg_type = resolve_arg_type(&runtime.heap, arg_type, &arg)?;
    eval_synthetic_application(runtime, func, func_type, &[(arg, arg_type)], gas, context).await
}

async fn eval_synthetic_application<State: Clone + Send + Sync + 'static>(
    runtime: &RuntimeSnapshot<State>,
    func: Pointer,
    func_type: Type,
    args: &[(Pointer, Type)],
    gas: &mut GasMeter,
    context: EvalContext,
) -> Result<Pointer, EngineError> {
    let (env, expr) = synthetic_application_expr(func, func_type, args)?;
    match context.parent {
        Some(parent) => eval_typed_expr_child(runtime, parent, &env, &expr, gas).await,
        None => eval_typed_expr(runtime, &env, &expr, gas).await,
    }
}

fn callable_pointer_type<State: Clone + Send + Sync + 'static>(
    runtime: &RuntimeSnapshot<State>,
    func: &Pointer,
) -> Result<Type, EngineError> {
    let value = runtime.heap.get(func)?;
    match value.as_ref() {
        Value::Closure(Closure { typ, .. }) => Ok(typ.clone()),
        Value::Native(native) => {
            let (_, _, _, typ, _, _, _) = native.clone().into_parts();
            Ok(typ)
        }
        Value::Overloaded(over) => {
            let (_, typ, _, _) = over.clone().into_parts();
            Ok(typ)
        }
        _ => Err(EngineError::NotCallable(
            runtime.heap.type_name(func)?.into(),
        )),
    }
}
