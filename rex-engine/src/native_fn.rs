use crate::{
    builder::registry::NativeId,
    error::EngineError,
    evaluator::{
        CallSite,
        context::Context,
        native_callable::{NativeCallResult, NativeCallable, SchedulerNativeResult},
        native_functions::NativeTask,
        resolve_arg_type,
        runtime_core::RuntimeCore,
    },
    handlers::NativeAsyncCall,
    util::{is_function_type, split_fun},
    value::{Collection, Pointer},
};
use rex_ast::Symbol;
use rex_typesystem::{
    types::{Type, Types},
    unification::unify,
};

pub(crate) enum NativeApplyResult<State: Clone + Send + Sync + 'static> {
    Value(Pointer),
    Task(NativeTask),
    Pending(NativeAsyncCall<State>),
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
    ) -> Result<NativeCallResult<State>, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        runtime.native_callable(self.native_id)?.call_at_site(
            runtime,
            self.typ.clone(),
            &[],
            call_site,
        )
    }

    pub(crate) fn apply_at_site<State: Clone + Send + Sync + 'static>(
        mut self,
        runtime: &RuntimeCore<State>,
        arg: Pointer,
        arg_type: Option<&Type>,
        call_site: CallSite,
    ) -> Result<NativeApplyResult<State>, EngineError> {
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
        let actual_ty = resolve_arg_type(&runtime.heap, arg_type, &arg)?;
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
            return runtime
                .heap
                .with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_native(native_id, name, arity, typ, applied, applied_types)?
                        .into_pointer())
                })
                .map(NativeApplyResult::Value);
        }

        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }

        match runtime.native_callable(self.native_id)? {
            NativeCallable::Scheduler(f) => {
                let ctx = Context::new_at_call_site(runtime, call_site);
                match f(ctx, full_ty, &self.applied)? {
                    SchedulerNativeResult::Ready(value) => Ok(NativeApplyResult::Value(value)),
                    SchedulerNativeResult::Task(task) => Ok(NativeApplyResult::Task(task)),
                }
            }
            callable => match callable.call_at_site(runtime, full_ty, &self.applied, call_site)? {
                NativeCallResult::Ready(value) => Ok(NativeApplyResult::Value(value)),
                NativeCallResult::Pending(future) => Ok(NativeApplyResult::Pending(future)),
            },
        }
    }
}

impl Collection for NativeFn {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.extend(self.applied.iter().copied());
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        for pointer in &mut self.applied {
            *pointer = rewrite(*pointer)?;
        }
        Ok(())
    }
}
