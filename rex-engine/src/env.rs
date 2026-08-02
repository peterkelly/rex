use std::collections::BTreeMap;
use std::sync::Arc;

use rex_ast::Symbol;

use crate::EngineError;
use crate::memory::{
    heap::{Handle, HeapState, PersistentPtr, PersistentRootStore, Pointer, RootScope},
    traits::Collection,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Environment(Arc<EnvEntry>);

#[derive(Default, Debug, PartialEq)]
struct EnvEntry {
    parent: Option<Environment>,
    bindings: BTreeMap<Symbol, Pointer>,
}

impl Environment {
    pub fn new() -> Self {
        Environment(Arc::new(EnvEntry::default()))
    }

    pub(crate) fn extend(&self, name: Symbol, value: Pointer) -> Self {
        let mut bindings = BTreeMap::new();
        bindings.insert(name, value);
        Environment(Arc::new(EnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn extend_many(&self, bindings: BTreeMap<Symbol, Pointer>) -> Self {
        Environment(Arc::new(EnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn get(&self, name: &Symbol) -> Option<Pointer> {
        let mut current: Option<&Environment> = Some(self);
        while let Some(env) = current {
            if let Some(v) = env.0.bindings.get(name) {
                return Some(*v);
            }
            current = env.0.parent.as_ref();
        }
        None
    }

    pub(crate) fn parent(&self) -> Option<&Environment> {
        self.0.parent.as_ref()
    }

    pub(crate) fn bindings(&self) -> &BTreeMap<Symbol, Pointer> {
        &self.0.bindings
    }
}

impl Collection for Environment {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        let mut entries = Vec::new();
        let mut current: Option<&Environment> = Some(self);
        while let Some(env) = current {
            let mut bindings = BTreeMap::new();
            for (name, pointer) in &env.0.bindings {
                bindings.insert(name.clone(), map(*pointer)?);
            }
            entries.push(bindings);
            current = env.0.parent.as_ref();
        }

        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(Environment(Arc::new(EnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }

        *self = rebuilt.unwrap_or_default();
        Ok(())
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

/// Evaluator-owned environment whose bindings remain valid while the heap is
/// unlocked. Heap closures continue to use `Environment`; only evaluator
/// frames use this persistent form.
#[derive(Debug, PartialEq)]
struct PersistentEnvEntry {
    parent: Option<PersistentEnvironment>,
    bindings: BTreeMap<Symbol, PersistentPtr>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PersistentEnvironment(Arc<PersistentEnvEntry>);

impl PersistentEnvironment {
    pub(crate) fn persist<'heap, 'scope>(
        env: Environment,
        roots: &mut PersistentRootStore,
        scope: &mut RootScope<'heap, 'scope>,
    ) -> Result<Self, EngineError> {
        let mut entries = Vec::new();
        let mut current = Some(&env);
        while let Some(entry) = current {
            let mut bindings = BTreeMap::new();
            for (name, pointer) in &entry.0.bindings {
                let rooted = scope.root(*pointer);
                bindings.insert(name.clone(), roots.insert(scope, rooted)?);
            }
            entries.push(bindings);
            current = entry.0.parent.as_ref();
        }

        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(PersistentEnvironment(Arc::new(PersistentEnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }
        Ok(rebuilt.unwrap_or_else(Self::new))
    }

    pub(crate) fn resolve<'heap, 'scope>(
        self,
        roots: &PersistentRootStore,
        scope: &mut RootScope<'heap, 'scope>,
    ) -> Result<Environment, EngineError> {
        let mut entries = Vec::new();
        let mut current = Some(&self);
        while let Some(entry) = current {
            let mut bindings = BTreeMap::new();
            for (name, value) in &entry.0.bindings {
                let rooted = roots.resolve(scope, value)?;
                bindings.insert(name.clone(), scope.pointer(rooted));
            }
            entries.push(bindings);
            current = entry.0.parent.as_ref();
        }

        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(Environment(Arc::new(EnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }
        Ok(rebuilt.unwrap_or_default())
    }

    fn new() -> Self {
        Self(Arc::new(PersistentEnvEntry {
            parent: None,
            bindings: BTreeMap::new(),
        }))
    }
}

struct RootedEnvEntry {
    parent: Option<RootedEnvironment>,
    bindings: BTreeMap<Symbol, Handle>,
}

#[derive(Clone)]
pub(crate) struct RootedEnvironment(Arc<RootedEnvEntry>);

impl RootedEnvironment {
    pub(crate) fn new() -> Self {
        RootedEnvironment(Arc::new(RootedEnvEntry {
            parent: None,
            bindings: BTreeMap::new(),
        }))
    }

    pub(crate) fn extend(&self, name: Symbol, value: Handle) -> Self {
        let mut bindings = BTreeMap::new();
        bindings.insert(name, value);
        RootedEnvironment(Arc::new(RootedEnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn get(&self, name: &Symbol) -> Option<Handle> {
        let mut current: Option<&RootedEnvironment> = Some(self);
        while let Some(env) = current {
            if let Some(v) = env.0.bindings.get(name) {
                return Some(v.clone());
            }
            current = env.0.parent.as_ref();
        }
        None
    }

    pub(crate) fn contains(&self, name: &Symbol) -> bool {
        let mut current: Option<&RootedEnvironment> = Some(self);
        while let Some(env) = current {
            if env.0.bindings.contains_key(name) {
                return true;
            }
            current = env.0.parent.as_ref();
        }
        false
    }

    pub(crate) fn to_environment(&self, heap: &HeapState) -> Result<Environment, EngineError> {
        let mut entries = Vec::new();
        let mut current = Some(self);
        while let Some(env) = current {
            let mut bindings = BTreeMap::new();
            for (name, handle) in &env.0.bindings {
                bindings.insert(name.clone(), handle.pointer(heap)?);
            }
            entries.push(bindings);
            current = env.0.parent.as_ref();
        }

        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(Environment(Arc::new(EnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }

        Ok(rebuilt.unwrap_or_default())
    }
}

impl Default for RootedEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::heap::Heap;

    #[test]
    fn rooted_environment_survives_copying_gc() {
        let heap = Heap::new();
        let a = heap.alloc_i32(1).unwrap();
        let b = heap.alloc_i32(2).unwrap();

        let rooted = RootedEnvironment::new()
            .extend(Symbol::intern("a"), a)
            .extend(Symbol::intern("b"), b);

        heap.set_collect_on_every_alloc(true).unwrap();
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                let root = scope.alloc_root_i32(3)?;
                Ok(scope.pointer(root))
            })
        })
        .unwrap();

        let env = heap
            .with_locked(|heap| rooted.to_environment(heap))
            .unwrap();
        let a = env.get(&Symbol::intern("a")).unwrap();
        let b = env.get(&Symbol::intern("b")).unwrap();
        assert_eq!(
            heap.with_locked(|heap| {
                heap.root_scope(|scope| {
                    let a = scope.root(a);
                    scope.root_as_i32(a)
                })
            })
            .unwrap(),
            1
        );
        assert_eq!(
            heap.with_locked(|heap| {
                heap.root_scope(|scope| {
                    let b = scope.root(b);
                    scope.root_as_i32(b)
                })
            })
            .unwrap(),
            2
        );
    }
}
