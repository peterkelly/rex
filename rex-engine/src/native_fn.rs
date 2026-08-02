use crate::{
    builder::registry::NativeId,
    error::EngineError,
    evaluator::{
        CallSite,
        context::InternalCtx,
        native_callable::{NativeCallable, SchedulerNativeResult},
        native_functions::NativeTask,
        resolve_arg_type,
        runtime_core::RuntimeCore,
    },
    handlers::NativeCallRequest,
    memory::{
        heap::{Pointer, RootScope, RootedPtr},
        traits::Collection,
    },
    util::{is_function_type, split_fun},
};
use rex_ast::Symbol;
use rex_typesystem::{
    types::{Type, Types},
    unification::unify,
};

pub(crate) enum NativeApplyResult<'scope, State: Clone + Send + Sync + 'static> {
    Value(RootedPtr<'scope>),
    Task(NativeTask<Pointer>),
    Pending(NativeCallRequest<State>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeFn {
    pub(crate) native_id: NativeId,
    pub(crate) name: Symbol,
    pub(crate) arity: usize,
    pub(crate) typ: Type,
    pub(crate) applied: Vec<Pointer>,
    pub(crate) applied_types: Vec<Type>,
}

impl NativeFn {
    pub(crate) fn new(native_id: NativeId, name: Symbol, arity: usize, typ: Type) -> Self {
        Self {
            native_id,
            name,
            arity,
            typ,
            applied: Vec::new(),
            applied_types: Vec::new(),
        }
    }

    pub(crate) fn from_parts(
        native_id: NativeId,
        name: Symbol,
        arity: usize,
        typ: Type,
        applied: Vec<Pointer>,
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

    pub(crate) fn into_parts(self) -> (NativeId, Symbol, usize, Type, Vec<Pointer>, Vec<Type>) {
        (
            self.native_id,
            self.name,
            self.arity,
            self.typ,
            self.applied,
            self.applied_types,
        )
    }

    pub(crate) fn name(&self) -> &Symbol {
        &self.name
    }

    pub(crate) fn call_zero_at_site<State: Clone + Send + Sync + 'static>(
        &self,
        runtime: &RuntimeCore<State>,
        call_site: CallSite,
    ) -> Result<NativeCallRequest<State>, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        runtime
            .native_callable(self.native_id)?
            .call_at_site(self.typ.clone(), &[], call_site)
    }

    pub(crate) fn apply_at_site<'scope, State: Clone + Send + Sync + 'static>(
        mut self,
        runtime: &RuntimeCore<State>,
        scope: &mut RootScope<'_, 'scope>,
        arg: Pointer,
        arg_type: Option<&Type>,
        call_site: CallSite,
    ) -> Result<NativeApplyResult<'scope, State>, EngineError> {
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
        let arg = scope.root(arg);
        let actual_ty = resolve_arg_type(scope, arg_type, arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        self.typ = rest_ty.apply(&subst);
        self.applied.push(scope.pointer(arg));
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
            let applied = applied.into_iter().map(|x| scope.root(x)).collect();
            let root =
                scope.alloc_root_native(native_id, name, arity, typ, applied, applied_types)?;
            return Ok(NativeApplyResult::Value(root));
        }

        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }

        match runtime.native_callable(self.native_id)? {
            NativeCallable::Scheduler(f) => {
                let ctx = InternalCtx::new_at_call_site(runtime, call_site);
                let applied = self
                    .applied
                    .iter()
                    .map(|x| scope.root(*x))
                    .collect::<Vec<_>>();
                match f(ctx, scope, full_ty, &applied)? {
                    SchedulerNativeResult::Ready(value) => Ok(NativeApplyResult::Value(value)),
                    SchedulerNativeResult::Task(task) => Ok(NativeApplyResult::Task(task)),
                }
            }
            callable => callable
                .call_at_site(full_ty, &self.applied, call_site)
                .map(NativeApplyResult::Pending),
        }
    }
}

impl Collection for NativeFn {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        for pointer in &mut self.applied {
            *pointer = map(*pointer)?;
        }
        Ok(())
    }
}
