use std::sync::Arc;

use rex_ast::Symbol;
use rex_typesystem::{
    error::TypeError,
    types::{Type, TypeKind, TypedExpr, Types},
    typesystem::TypeSystem,
    unification::{Subst, unify},
};

use crate::{
    builder::registry::NativeImpl,
    env::Environment,
    error::EngineError,
    evaluator::{CallSite, runtime_core::RuntimeCore},
    overloaded_fn::OverloadedFn,
    util::{impl_matches_type, is_function_type},
    value::{Handle, Heap, Pointer},
};

#[derive(Clone)]
pub struct Context<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    runtime: RuntimeCore<State>,
    #[allow(dead_code)]
    #[doc(hidden)]
    pub(crate) call_site: CallSite,
}

impl<State> Context<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new_at_call_site(runtime: &RuntimeCore<State>, call_site: CallSite) -> Self {
        Self {
            runtime: runtime.clone(),
            call_site,
        }
    }

    pub(crate) fn new_with_parent(runtime: &RuntimeCore<State>, parent: Pointer) -> Self {
        Self::new_at_call_site(runtime, CallSite::child(parent))
    }

    pub fn state(&self) -> &State {
        self.runtime.state.as_ref()
    }

    pub fn heap(&self) -> &Heap {
        &self.runtime.heap
    }

    pub fn type_system(&self) -> &TypeSystem {
        self.runtime.type_system.as_ref()
    }

    pub(crate) fn handles_from_pointers(
        &self,
        pointers: &[Pointer],
    ) -> Result<Vec<Handle>, EngineError> {
        pointers
            .iter()
            .map(|pointer| self.runtime.heap.handle(*pointer))
            .collect()
    }

    fn resolve_typeclass_method_impl(
        &self,
        name: &Symbol,
        call_type: &Type,
    ) -> Result<(Environment, Arc<TypedExpr>, Subst), EngineError> {
        let info = self
            .runtime
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

        self.runtime
            .typeclasses
            .resolve(&info.class, name, &param_type)
    }

    pub(crate) fn cached_class_method(&self, name: &Symbol, typ: &Type) -> Option<Pointer> {
        if !typ.ftv().is_empty() {
            return None;
        }
        let cache = self.runtime.typeclass_cache.lock().ok()?;
        cache.get(&(name.clone(), typ.clone())).cloned()
    }

    pub(crate) fn resolve_class_method_plan(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<Result<(Environment, TypedExpr), Pointer>, EngineError> {
        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                let (name, typ, applied, applied_types) =
                    OverloadedFn::new(name.clone(), typ.clone()).into_parts();
                let pointer =
                    self.runtime
                        .heap
                        .alloc_ptr_overloaded(name, typ, applied, applied_types)?;
                return Ok(Err(pointer));
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        Ok(Ok((def_env, specialized)))
    }

    pub(crate) fn resolve_native_impl(
        &self,
        name: &str,
        typ: &Type,
    ) -> Result<NativeImpl<State>, EngineError> {
        let sym_name = Symbol::intern(name);
        let impls = self
            .runtime
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => Ok(matches[0].clone()),
            _ => Err(EngineError::AmbiguousImpl {
                name: sym_name,
                typ: typ.to_string(),
            }),
        }
    }

    pub(crate) fn resolve_native(&self, name: &str, typ: &Type) -> Result<Pointer, EngineError> {
        let sym_name = Symbol::intern(name);
        let impls = self
            .runtime
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => {
                let imp = matches[0].clone();
                let (native_id, name, arity, typ, applied, applied_types) =
                    imp.to_native_fn(typ.clone()).into_parts();
                self.runtime.heap.alloc_ptr_native(
                    native_id,
                    name,
                    arity,
                    typ,
                    applied,
                    applied_types,
                )
            }
            _ => {
                if typ.ftv().is_empty() {
                    Err(EngineError::AmbiguousImpl {
                        name: sym_name.clone(),
                        typ: typ.to_string(),
                    })
                } else if is_function_type(typ) {
                    let (name, typ, applied, applied_types) =
                        OverloadedFn::new(sym_name.clone(), typ.clone()).into_parts();
                    self.runtime
                        .heap
                        .alloc_ptr_overloaded(name, typ, applied, applied_types)
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
