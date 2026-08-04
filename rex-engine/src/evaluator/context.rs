use std::sync::Arc;

use rex_ast::Symbol;
use rex_typesystem::{
    error::TypeError,
    types::{Type, TypeKind, TypedExpr, Types},
    typesystem::TypeSystem,
    unification::{Subst, unify},
};

use crate::{
    builder::registry::{NativeId, NativeResolution},
    env::{RootedEnvironment, ScopedEnvironment},
    error::EngineError,
    evaluator::runtime_core::RuntimeCore,
    memory::heap::{RootScope, RootedPtr},
    util::is_function_type,
};

/// Heap-independent context supplied to host callbacks.
#[derive(Clone)]
pub struct Context<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    state: Arc<State>,
    type_system: Arc<TypeSystem>,
}

impl<State> Context<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(runtime: &RuntimeCore<State>) -> Self {
        Self {
            state: Arc::clone(&runtime.state),
            type_system: Arc::clone(&runtime.type_system),
        }
    }

    pub fn state(&self) -> &State {
        self.state.as_ref()
    }

    pub fn type_system(&self) -> &TypeSystem {
        self.type_system.as_ref()
    }
}

pub(crate) enum ClassMethodPlan {
    Evaluate {
        env: ScopedEnvironment,
        expr: TypedExpr,
    },
    Deferred(RootedPtr),
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

    pub(crate) fn resolve_class_method_plan(
        &self,
        scope: &mut RootScope<'_>,
        name: &Symbol,
        typ: &Type,
    ) -> Result<ClassMethodPlan, EngineError> {
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
            env: def_env.to_scoped_environment(),
            expr: specialized,
        })
    }

    pub(crate) fn resolve_native_parts(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<(NativeId, Symbol, usize), EngineError> {
        let (native_id, arity) = self.natives.resolve_unique(name, typ)?;
        Ok((native_id, name.clone(), arity))
    }

    pub(crate) fn resolve_native(
        &self,
        scope: &mut RootScope<'_>,
        name: &Symbol,
        typ: &Type,
    ) -> Result<RootedPtr, EngineError> {
        match self.natives.resolve(name, typ)? {
            NativeResolution::Unique { native_id, arity } => {
                let root = scope.alloc_root_native(
                    native_id,
                    name.clone(),
                    arity,
                    typ.clone(),
                    Vec::new(),
                    Vec::new(),
                )?;
                Ok(root)
            }
            NativeResolution::Ambiguous => {
                if typ.ftv().is_empty() {
                    Err(EngineError::AmbiguousImpl {
                        name: name.clone(),
                        typ: typ.to_string(),
                    })
                } else if is_function_type(typ) {
                    let root = scope.alloc_root_overloaded(
                        name.clone(),
                        typ.clone(),
                        Vec::new(),
                        Vec::new(),
                    )?;
                    Ok(root)
                } else {
                    Err(EngineError::AmbiguousOverload { name: name.clone() })
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
