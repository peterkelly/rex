use std::{collections::BTreeMap, sync::Arc};

use rex_ast::Symbol;
use rex_typesystem::{
    types::{Scheme, Type, TypedExpr},
    typesystem::{TypeVarSupply, instantiate},
    unification::{Subst, unify},
};

use crate::{
    env::RootedEnvironment, error::EngineError, evaluator::native_callable::NativeCallable,
};

pub(crate) type NativeId = u64;

#[derive(Clone)]
struct NativeImpl<State: Clone + Send + Sync + 'static> {
    id: NativeId,
    arity: usize,
    scheme: Scheme,
    func: NativeCallable<State>,
}

/// Result of matching a native name and call type against registered schemes.
///
/// Ambiguity remains explicit because function-valued overloads can defer the
/// final choice until their arguments provide enough type information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeResolution {
    Unique { native_id: NativeId, arity: usize },
    Ambiguous,
}

#[derive(Clone)]
pub(crate) struct NativeRegistry<State: Clone + Send + Sync + 'static> {
    next_id: NativeId,
    entries: BTreeMap<Symbol, Vec<NativeImpl<State>>>,
    by_id: BTreeMap<NativeId, NativeImpl<State>>,
}

impl<State: Clone + Send + Sync + 'static> NativeRegistry<State> {
    pub(crate) fn insert(
        &mut self,
        name: Symbol,
        arity: usize,
        scheme: Scheme,
        func: NativeCallable<State>,
    ) -> Result<(), EngineError> {
        let entry = self.entries.entry(name.clone()).or_default();
        if entry.iter().any(|existing| existing.scheme == scheme) {
            return Err(EngineError::DuplicateImpl {
                name,
                typ: scheme.typ.to_string(),
            });
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let imp = NativeImpl::<State> {
            id,
            arity,
            scheme,
            func,
        };
        self.by_id.insert(id, imp.clone());
        entry.push(imp);
        Ok(())
    }

    pub(crate) fn has_name(&self, name: &Symbol) -> bool {
        self.entries.contains_key(name)
    }

    pub(crate) fn contains_scheme(&self, name: &Symbol, scheme: &Scheme) -> bool {
        self.entries
            .get(name)
            .is_some_and(|impls| impls.iter().any(|imp| &imp.scheme == scheme))
    }

    pub(crate) fn resolve(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<NativeResolution, EngineError> {
        let impls = self
            .entries
            .get(name)
            .ok_or_else(|| EngineError::UnknownVar(name.clone()))?;
        let mut matches = impls.iter().filter(|imp| impl_matches_type(imp, typ));
        let Some(imp) = matches.next() else {
            return Err(EngineError::MissingImpl {
                name: name.clone(),
                typ: typ.to_string(),
            });
        };
        if matches.next().is_some() {
            Ok(NativeResolution::Ambiguous)
        } else {
            Ok(NativeResolution::Unique {
                native_id: imp.id,
                arity: imp.arity,
            })
        }
    }

    pub(crate) fn resolve_unique(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<(NativeId, usize), EngineError> {
        match self.resolve(name, typ)? {
            NativeResolution::Unique { native_id, arity } => Ok((native_id, arity)),
            NativeResolution::Ambiguous => Err(EngineError::AmbiguousImpl {
                name: name.clone(),
                typ: typ.to_string(),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn schemes(&self) -> impl Iterator<Item = (&Symbol, Vec<Scheme>)> {
        self.entries
            .iter()
            .map(|(name, impls)| (name, impls.iter().map(|imp| imp.scheme.clone()).collect()))
    }

    pub(crate) fn callable_by_id(&self, id: NativeId) -> Option<&NativeCallable<State>> {
        self.by_id.get(&id).map(|imp| &imp.func)
    }
}

fn impl_matches_type<State: Clone + Send + Sync + 'static>(
    imp: &NativeImpl<State>,
    typ: &Type,
) -> bool {
    let mut supply = TypeVarSupply::new();
    let (_preds, scheme_ty) = instantiate(&imp.scheme, &mut supply);
    unify(&scheme_ty, typ).is_ok()
}

impl<State: Clone + Send + Sync + 'static> Default for NativeRegistry<State> {
    fn default() -> Self {
        Self {
            next_id: 0,
            entries: BTreeMap::new(),
            by_id: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct TypeclassInstance {
    head: Type,
    def_env: RootedEnvironment,
    methods: BTreeMap<Symbol, Arc<TypedExpr>>,
}

#[derive(Default, Clone)]
pub(crate) struct TypeclassRegistry {
    entries: BTreeMap<Symbol, Vec<TypeclassInstance>>,
}

impl TypeclassRegistry {
    pub(crate) fn insert(
        &mut self,
        class: Symbol,
        head: Type,
        def_env: RootedEnvironment,
        methods: BTreeMap<Symbol, Arc<TypedExpr>>,
    ) -> Result<(), EngineError> {
        let entry = self.entries.entry(class.clone()).or_default();
        for existing in entry.iter() {
            if unify(&existing.head, &head).is_ok() {
                return Err(EngineError::DuplicateTypeclassImpl {
                    class,
                    typ: head.to_string(),
                });
            }
        }
        entry.push(TypeclassInstance {
            head,
            def_env,
            methods,
        });
        Ok(())
    }

    pub(crate) fn resolve<'a>(
        &'a self,
        class: &Symbol,
        method: &Symbol,
        param_type: &Type,
    ) -> Result<(&'a RootedEnvironment, &'a Arc<TypedExpr>, Subst), EngineError> {
        let instances =
            self.entries
                .get(class)
                .ok_or_else(|| EngineError::MissingTypeclassImpl {
                    class: class.clone(),
                    typ: param_type.to_string(),
                })?;

        let mut matches = Vec::new();
        for inst in instances {
            if let Ok(s) = unify(&inst.head, param_type) {
                matches.push((inst, s));
            }
        }
        match matches.len() {
            0 => Err(EngineError::MissingTypeclassImpl {
                class: class.clone(),
                typ: param_type.to_string(),
            }),
            1 => {
                let (inst, s) = matches.remove(0);
                let typed =
                    inst.methods
                        .get(method)
                        .ok_or_else(|| EngineError::MissingTypeclassImpl {
                            class: class.clone(),
                            typ: param_type.to_string(),
                        })?;
                Ok((&inst.def_env, typed, s))
            }
            _ => Err(EngineError::AmbiguousTypeclassImpl {
                class: class.clone(),
                typ: param_type.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use rex_typesystem::types::{BuiltinTypeId, TypeVar};

    use super::*;

    fn unused_native() -> NativeCallable<()> {
        NativeCallable::Scheduler(Arc::new(|_, _, _| unreachable!()))
    }

    #[test]
    fn native_resolution_classifies_all_match_outcomes() {
        let mut registry = NativeRegistry::default();
        let name = Symbol::intern("choose");
        let i32_ty = Type::builtin(BuiltinTypeId::I32);
        let bool_ty = Type::builtin(BuiltinTypeId::Bool);
        let concrete = Scheme::new(
            Vec::new(),
            Vec::new(),
            Type::fun(i32_ty.clone(), i32_ty.clone()),
        );
        let var = TypeVar::new(0, None::<Symbol>);
        let generic = Scheme::new(
            vec![var.clone()],
            Vec::new(),
            Type::fun(Type::var(var.clone()), Type::var(var)),
        );

        registry
            .insert(name.clone(), 1, concrete, unused_native())
            .unwrap();
        registry
            .insert(name.clone(), 1, generic, unused_native())
            .unwrap();

        let bool_fn = Type::fun(bool_ty.clone(), bool_ty);
        let NativeResolution::Unique { native_id, arity } =
            registry.resolve(&name, &bool_fn).unwrap()
        else {
            panic!("expected one matching native implementation");
        };
        assert_eq!(native_id, 1);
        assert_eq!(arity, 1);

        let i32_fn = Type::fun(i32_ty.clone(), i32_ty.clone());
        assert!(matches!(
            registry.resolve(&name, &i32_fn),
            Ok(NativeResolution::Ambiguous)
        ));
        assert!(matches!(
            registry.resolve_unique(&name, &i32_fn),
            Err(EngineError::AmbiguousImpl { .. })
        ));
        assert!(matches!(
            registry.resolve(&name, &i32_ty),
            Err(EngineError::MissingImpl { .. })
        ));
        assert!(matches!(
            registry.resolve(&Symbol::intern("missing"), &i32_ty),
            Err(EngineError::UnknownVar(_))
        ));
    }
}
