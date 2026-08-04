use crate::{
    builder::registry::NativeId,
    error::EngineError,
    evaluator::{
        native_callable::{NativeCallable, SchedulerNativeResult},
        native_functions::NativeTask,
        resolve_arg_type,
        runtime_core::RuntimeCore,
    },
    handlers::NativeCallRequest,
    memory::heap::{RootScope, RootedPtr},
    util::{is_function_type, split_fun},
};
use rex_ast::Symbol;
use rex_typesystem::{
    types::{Type, Types},
    unification::unify,
};

pub(crate) enum NativeApplyResult {
    Value(RootedPtr),
    Task(NativeTask<RootedPtr>),
    Pending(NativeCallRequest),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeFn<P> {
    pub(crate) native_id: NativeId,
    pub(crate) name: Symbol,
    pub(crate) arity: usize,
    pub(crate) typ: Type,
    pub(crate) applied: Vec<P>,
    pub(crate) applied_types: Vec<Type>,
}

impl<P> NativeFn<P> {
    pub(crate) fn from_parts(
        native_id: NativeId,
        name: Symbol,
        arity: usize,
        typ: Type,
        applied: Vec<P>,
        applied_types: Vec<Type>,
    ) -> Self {
        Self {
            native_id,
            name,
            arity,
            typ,
            applied,
            applied_types,
        }
    }
}

impl NativeFn<RootedPtr> {
    pub(crate) fn call_zero<State: Clone + Send + Sync + 'static>(
        &self,
        runtime: &RuntimeCore<State>,
        scope: &mut RootScope<'_>,
    ) -> Result<NativeApplyResult, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        match runtime.native_callable(self.native_id)? {
            NativeCallable::Constant(value) => Ok(NativeApplyResult::Value(*value)),
            NativeCallable::Scheduler(callable) => match callable(scope, self.typ.clone(), &[])? {
                SchedulerNativeResult::Ready(value) => Ok(NativeApplyResult::Value(value)),
                SchedulerNativeResult::Task(task) => Ok(NativeApplyResult::Task(task)),
            },
            callable => callable
                .call(self.native_id, self.typ.clone(), &[])
                .map(NativeApplyResult::Pending),
        }
    }

    pub(crate) fn apply<State: Clone + Send + Sync + 'static>(
        mut self,
        runtime: &RuntimeCore<State>,
        scope: &mut RootScope<'_>,
        arg: RootedPtr,
        arg_type: Option<&Type>,
    ) -> Result<NativeApplyResult, EngineError> {
        // `self` is an owned copy cloned from heap storage; we mutate it to
        // accumulate partial-application state and never mutate shared values.
        if self.arity == 0 {
            return Err(EngineError::NativeArity {
                name: self.name,
                expected: 0,
                got: 1,
            });
        }
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(scope, arg_type, arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        self.typ = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&self.typ) {
            let NativeFn {
                native_id,
                name,
                arity,
                typ,
                applied,
                applied_types,
            } = self;
            let root =
                scope.alloc_root_native(native_id, name, arity, typ, applied, applied_types)?;
            return Ok(NativeApplyResult::Value(root));
        }

        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }

        match runtime.native_callable(self.native_id)? {
            NativeCallable::Constant(_) => Err(EngineError::NativeArity {
                name: self.name,
                expected: 0,
                got: self.applied.len(),
            }),
            NativeCallable::Scheduler(f) => match f(scope, full_ty, &self.applied)? {
                SchedulerNativeResult::Ready(value) => Ok(NativeApplyResult::Value(value)),
                SchedulerNativeResult::Task(task) => Ok(NativeApplyResult::Task(task)),
            },
            callable => callable
                .call(self.native_id, full_ty, &self.applied)
                .map(NativeApplyResult::Pending),
        }
    }
}
