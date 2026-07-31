use std::{collections::BTreeMap, sync::Arc};

use rex_ast::Symbol;
use rex_typesystem::{
    types::{Scheme, Type, TypedExpr},
    unification::{Subst, unify},
};

use crate::{
    env::{Environment, RootedEnvironment},
    error::EngineError,
    evaluator::native_callable::NativeCallable,
    memory::heap::HeapState,
    native_fn::NativeFn,
};

pub(crate) type NativeId = u64;

#[derive(Clone)]
pub(crate) struct NativeImpl<State: Clone + Send + Sync + 'static> {
    id: NativeId,
    name: Symbol,
    pub(crate) arity: usize,
    pub(crate) scheme: Scheme,
    pub(crate) func: NativeCallable<State>,
}

impl<State: Clone + Send + Sync + 'static> NativeImpl<State> {
    pub(crate) fn to_native_fn(&self, typ: Type) -> NativeFn {
        NativeFn::new(self.id, self.name.clone(), self.arity, typ)
    }
}

#[derive(Clone)]
pub(crate) struct NativeRegistry<State: Clone + Send + Sync + 'static> {
    next_id: NativeId,
    pub(crate) entries: BTreeMap<Symbol, Vec<NativeImpl<State>>>,
    pub(crate) by_id: BTreeMap<NativeId, NativeImpl<State>>,
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
            name: name.clone(),
            arity,
            scheme,
            func,
        };
        self.by_id.insert(id, imp.clone());
        entry.push(imp);
        Ok(())
    }

    pub(crate) fn get(&self, name: &Symbol) -> Option<&[NativeImpl<State>]> {
        self.entries.get(name).map(|v| v.as_slice())
    }

    pub(crate) fn has_name(&self, name: &Symbol) -> bool {
        self.entries.contains_key(name)
    }

    pub(crate) fn by_id(&self, id: NativeId) -> Option<&NativeImpl<State>> {
        self.by_id.get(&id)
    }
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

    pub(crate) fn resolve(
        &self,
        class: &Symbol,
        method: &Symbol,
        param_type: &Type,
        heap: &HeapState,
    ) -> Result<(Environment, Arc<TypedExpr>, Subst), EngineError> {
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
                Ok((inst.def_env.to_environment(heap)?, typed.clone(), s))
            }
            _ => Err(EngineError::AmbiguousTypeclassImpl {
                class: class.clone(),
                typ: param_type.to_string(),
            }),
        }
    }
}
