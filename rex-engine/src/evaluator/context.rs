use std::sync::Arc;

use rex_ast::Symbol;
use rex_typesystem::{
    error::TypeError,
    types::{Type, TypeKind, TypedExpr, Types},
    typesystem::TypeSystem,
    unification::{Subst, unify},
};

use crate::{
    builder::registry::{NativeId, NativeImpl},
    env::{RootedEnvironment, ScopedEnvironment},
    error::EngineError,
    evaluator::{
        CallSite, application_result_type, eval::eval_typed_expr, runtime_core::RuntimeCore,
    },
    memory::heap::{Handle, Heap, RootScope, RootedPtr},
    util::{impl_matches_type, is_function_type},
};

/// Public context supplied to handle-based host callbacks.
///
/// The evaluator invokes those callbacks only after releasing its locked
/// synchronous heap cycle. The context therefore exposes the shared [`Heap`]
/// for allocating rooted [`Handle`] values, along with immutable runtime state
/// and type information. It never exposes an internal moving pointer.
#[derive(Clone)]
pub struct Context<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) inner: InternalCtx<State>,
    heap: Heap,
}

impl<State> Context<State>
where
    State: Clone + Send + Sync + 'static,
{
    /// Pair an internal evaluator context with its public heap capability.
    ///
    /// Host callbacks receive a `Context` from the evaluator and do not need
    /// to call this constructor. It exists for follow-up host work after
    /// [`Evaluator::run_with_context`](crate::Evaluator::run_with_context).
    /// `heap` must be the heap from that same evaluator lineage; use the heap
    /// of the returned [`Handle`] or clone [`Evaluator::heap`](crate::Evaluator::heap)
    /// before consuming the evaluator. Pairing an internal context with an
    /// unrelated heap causes later callback evaluation to reject foreign-heap
    /// handles.
    pub fn new(inner: InternalCtx<State>, heap: Heap) -> Self {
        Self { inner, heap }
    }

    /// Shared heap for allocating or inspecting public rooted values.
    ///
    /// Heap operations may lock and collect. They are safe in a host callback
    /// because the evaluator releases its own heap guard before invoking it.
    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    pub fn state(&self) -> &State {
        self.inner.runtime.state.as_ref()
    }

    pub fn type_system(&self) -> &TypeSystem {
        self.inner.runtime.type_system.as_ref()
    }
}

#[derive(Clone)]
pub struct InternalCtx<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    runtime: RuntimeCore<State>,
    #[allow(dead_code)]
    #[doc(hidden)]
    pub(crate) call_site: CallSite,
}

pub(crate) enum ClassMethodPlan<'scope> {
    Evaluate {
        env: ScopedEnvironment<'scope>,
        expr: TypedExpr,
    },
    Deferred(RootedPtr<'scope>),
}

impl<State> InternalCtx<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new_at_call_site(runtime: &RuntimeCore<State>, call_site: CallSite) -> Self {
        Self {
            runtime: runtime.clone(),
            call_site,
        }
    }

    pub fn state(&self) -> &State {
        self.runtime.state.as_ref()
    }

    pub fn type_system(&self) -> &TypeSystem {
        self.runtime.type_system.as_ref()
    }

    /// Resume one Rex callback from engine-owned host action machinery.
    ///
    /// This is the internal callback bridge used by [`run_host_action`](super::host_action::run_host_action).
    /// Host-managed action values, such as `std.io.IO`, may store Rex callbacks
    /// in `map`, `ap`, or `bind` nodes. When the host action runner reaches one
    /// of those nodes, it needs to evaluate exactly one ordinary Rex function
    /// application against already-produced Rex values. This helper constructs a
    /// small synthetic typed application expression rooted in the current heap
    /// and hands that expression back to the normal evaluator.
    ///
    /// The important constraint is that this function resumes a callback once;
    /// it must not be used as a recursive action interpreter. In particular, a
    /// monadic runner must not call this function and then immediately recurse
    /// into itself to execute the callback's returned action. That shape rebuilds
    /// user-authored recursion in Rust async call frames, so a deeply nested Rex
    /// `bind` chain could overflow the Rust stack despite the evaluator's own
    /// iterative machinery.
    ///
    /// The intended pattern is the one used by `run_host_action`: keep the host
    /// control flow in an explicit loop with an explicit continuation stack. A
    /// `map` callback may continue within the same loop because it produces an
    /// ordinary value; a `bind` callback produces the next action handle and the
    /// runner returns that handle to its outer loop instead of recursively
    /// awaiting another action run. Keeping this helper crate-private makes that
    /// discipline enforceable: embedders get a narrow host-action API rather
    /// than a general "apply arbitrary Rex function" escape hatch.
    pub(crate) async fn resume_callback_once(
        &self,
        func: Handle,
        func_type: Type,
        args: Vec<(Handle, Type)>,
        heap: &Heap,
    ) -> Result<Handle, EngineError> {
        func.ensure_heap(heap)?;
        let func_name = Symbol::intern("__rex_apply_func");
        let mut env = RootedEnvironment::new().extend(func_name.clone(), func);
        let mut expr = TypedExpr::new(
            func_type.clone(),
            rex_typesystem::types::TypedExprKind::Var {
                name: func_name,
                overloads: Vec::new(),
            },
        );
        let mut cur_type = func_type;

        for (idx, (arg, arg_type)) in args.into_iter().enumerate() {
            arg.ensure_heap(heap)?;
            let arg_name = Symbol::intern(&format!("__rex_apply_arg_{idx}"));
            env = env.extend(arg_name.clone(), arg);
            let arg_expr = TypedExpr::new(
                arg_type.clone(),
                rex_typesystem::types::TypedExprKind::Var {
                    name: arg_name,
                    overloads: Vec::new(),
                },
            );
            let result_type = application_result_type(&cur_type, &arg_type)?;
            expr = TypedExpr::new(
                result_type.clone(),
                rex_typesystem::types::TypedExprKind::App(Arc::new(expr), Arc::new(arg_expr)),
            );
            cur_type = result_type;
        }

        eval_typed_expr(self.runtime.clone(), heap, env, Arc::new(expr), Vec::new()).await
    }
}

impl<State> RuntimeCore<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn resolve_typeclass_method_impl(
        &self,
        name: &Symbol,
        call_type: &Type,
    ) -> Result<(&RootedEnvironment, &Arc<TypedExpr>, Subst), EngineError> {
        let info = self
            .type_system
            .class_methods
            .get(name)
            .ok_or_else(|| EngineError::UnknownVar(name.clone()))?;

        let s_method = unify(&info.scheme.typ, call_type).map_err(EngineError::Type)?;
        let class_pred = info
            .scheme
            .preds
            .iter()
            .find(|p| p.class == info.class)
            .ok_or(EngineError::Type(TypeError::UnsupportedExpr(
                "method scheme missing class predicate",
            )))?;
        let param_type = class_pred.typ.apply(&s_method);
        if type_head_is_var(&param_type) {
            return Err(EngineError::AmbiguousOverload { name: name.clone() });
        }

        self.typeclasses.resolve(&info.class, name, &param_type)
    }

    pub(crate) fn resolve_class_method_plan<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        name: &Symbol,
        typ: &Type,
    ) -> Result<ClassMethodPlan<'scope>, EngineError> {
        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                let root = scope.alloc_root_overloaded(
                    name.clone(),
                    typ.clone(),
                    Vec::new(),
                    Vec::new(),
                )?;
                return Ok(ClassMethodPlan::Deferred(root));
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        Ok(ClassMethodPlan::Evaluate {
            env: def_env.to_scoped_environment(scope)?,
            expr: specialized,
        })
    }

    pub(crate) fn resolve_native_parts(
        &self,
        name: &str,
        typ: &Type,
    ) -> Result<(NativeId, Symbol, usize), EngineError> {
        let sym_name = Symbol::intern(name);
        let impls = self
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<&NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => Ok(matches[0].runtime_parts()),
            _ => Err(EngineError::AmbiguousImpl {
                name: sym_name,
                typ: typ.to_string(),
            }),
        }
    }

    pub(crate) fn resolve_native<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        name: &str,
        typ: &Type,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        let sym_name = Symbol::intern(name);
        let impls = self
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<&NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => {
                let imp = matches[0];
                let (native_id, name, arity) = imp.runtime_parts();
                let root = scope.alloc_root_native(
                    native_id,
                    name,
                    arity,
                    typ.clone(),
                    Vec::new(),
                    Vec::new(),
                )?;
                Ok(root)
            }
            _ => {
                if typ.ftv().is_empty() {
                    Err(EngineError::AmbiguousImpl {
                        name: sym_name.clone(),
                        typ: typ.to_string(),
                    })
                } else if is_function_type(typ) {
                    let root = scope.alloc_root_overloaded(
                        sym_name.clone(),
                        typ.clone(),
                        Vec::new(),
                        Vec::new(),
                    )?;
                    Ok(root)
                } else {
                    Err(EngineError::AmbiguousOverload { name: sym_name })
                }
            }
        }
    }
}

fn type_head_is_var(typ: &Type) -> bool {
    let mut cur = typ;
    while let TypeKind::App(head, _) = cur.as_ref() {
        cur = head;
    }
    matches!(cur.as_ref(), TypeKind::Var(..))
}
