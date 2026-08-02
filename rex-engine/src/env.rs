use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use rex_ast::Symbol;

use crate::EngineError;
use crate::memory::heap::{Handle, PersistentPtr, PersistentRootStore, RootScope, RootedPtr};

#[derive(Debug, PartialEq)]
struct ScopedEnvEntry<'scope> {
    parent: Option<ScopedEnvironment<'scope>>,
    bindings: BTreeMap<Symbol, RootedPtr<'scope>>,
}

/// Environment used while a synchronous evaluator cycle owns a `RootScope`.
///
/// Unlike the environment embedded in heap closures, every value here is a
/// shadow-stack root and is therefore rewritten automatically by collection.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedEnvironment<'scope>(Rc<ScopedEnvEntry<'scope>>);

impl<'scope> ScopedEnvironment<'scope> {
    pub(crate) fn new() -> Self {
        Self(Rc::new(ScopedEnvEntry {
            parent: None,
            bindings: BTreeMap::new(),
        }))
    }

    pub(crate) fn entries(&self) -> Vec<BTreeMap<Symbol, RootedPtr<'scope>>> {
        let mut entries = Vec::new();
        let mut current = Some(self);
        while let Some(entry) = current {
            entries.push(
                entry
                    .0
                    .bindings
                    .iter()
                    .map(|(name, value)| (name.clone(), *value))
                    .collect(),
            );
            current = entry.0.parent.as_ref();
        }
        entries
    }

    pub(crate) fn extend(&self, name: Symbol, value: RootedPtr<'scope>) -> Self {
        let mut bindings = BTreeMap::new();
        bindings.insert(name, value);
        Self(Rc::new(ScopedEnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn extend_many(&self, bindings: BTreeMap<Symbol, RootedPtr<'scope>>) -> Self {
        Self(Rc::new(ScopedEnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn get(&self, name: &Symbol) -> Option<RootedPtr<'scope>> {
        let mut current = Some(self);
        while let Some(env) = current {
            if let Some(value) = env.0.bindings.get(name) {
                return Some(*value);
            }
            current = env.0.parent.as_ref();
        }
        None
    }

    pub(crate) fn from_entries(entries: Vec<BTreeMap<Symbol, RootedPtr<'scope>>>) -> Self {
        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(Self(Rc::new(ScopedEnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }
        rebuilt.unwrap_or_else(Self::new)
    }
}

/// Evaluator-owned environment whose bindings remain valid while the heap is
/// unlocked.
#[derive(Debug, PartialEq)]
struct PersistentEnvEntry {
    parent: Option<PersistentEnvironment>,
    bindings: BTreeMap<Symbol, PersistentPtr>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PersistentEnvironment(Arc<PersistentEnvEntry>);

impl PersistentEnvironment {
    pub(crate) fn persist<'heap, 'scope>(
        env: ScopedEnvironment<'scope>,
        roots: &mut PersistentRootStore,
        scope: &mut RootScope<'heap, 'scope>,
    ) -> Result<Self, EngineError> {
        let mut entries = Vec::new();
        let mut current = Some(&env);
        while let Some(entry) = current {
            let mut bindings = BTreeMap::new();
            for (name, value) in &entry.0.bindings {
                bindings.insert(name.clone(), roots.insert(scope, *value)?);
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
    ) -> Result<ScopedEnvironment<'scope>, EngineError> {
        let mut entries = Vec::new();
        let mut current = Some(&self);
        while let Some(entry) = current {
            let mut bindings = BTreeMap::new();
            for (name, value) in &entry.0.bindings {
                let rooted = roots.resolve(scope, value)?;
                bindings.insert(name.clone(), rooted);
            }
            entries.push(bindings);
            current = entry.0.parent.as_ref();
        }

        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(ScopedEnvironment(Rc::new(ScopedEnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }
        Ok(rebuilt.unwrap_or_else(ScopedEnvironment::new))
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

    pub(crate) fn to_scoped_environment<'heap, 'scope>(
        &self,
        scope: &mut RootScope<'heap, 'scope>,
    ) -> Result<ScopedEnvironment<'scope>, EngineError> {
        let mut entries = Vec::new();
        let mut current = Some(self);
        while let Some(env) = current {
            let mut bindings = BTreeMap::new();
            for (name, handle) in &env.0.bindings {
                bindings.insert(name.clone(), scope.root_handle(handle)?);
            }
            entries.push(bindings);
            current = env.0.parent.as_ref();
        }

        Ok(ScopedEnvironment::from_entries(entries))
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
        heap.with_root_scope(|scope| {
            let env = rooted.to_scoped_environment(scope)?;
            scope.alloc_root_i32(3)?;
            let a = env.get(&Symbol::intern("a")).unwrap();
            let b = env.get(&Symbol::intern("b")).unwrap();
            assert_eq!(scope.root_as_i32(a)?, 1);
            assert_eq!(scope.root_as_i32(b)?, 2);
            Ok(())
        })
        .unwrap();
    }
}
