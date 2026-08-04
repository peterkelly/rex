use std::collections::BTreeMap;
use std::sync::Arc;

use rex_ast::Symbol;

use crate::memory::heap::RootedPtr;

#[derive(Debug, PartialEq)]
struct ScopedEnvEntry {
    parent: Option<ScopedEnvironment>,
    bindings: BTreeMap<Symbol, RootedPtr>,
}

/// Environment used while a synchronous evaluator cycle owns a `RootScope`.
///
/// Unlike the environment embedded in heap closures, every value here is a
/// shadow-stack root and is therefore rewritten automatically by collection.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedEnvironment(Arc<ScopedEnvEntry>);

impl ScopedEnvironment {
    pub(crate) fn new() -> Self {
        Self(Arc::new(ScopedEnvEntry {
            parent: None,
            bindings: BTreeMap::new(),
        }))
    }

    pub(crate) fn entries(&self) -> Vec<BTreeMap<Symbol, RootedPtr>> {
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

    pub(crate) fn visit_values(&self, visit: &mut impl FnMut(RootedPtr)) {
        let mut current = Some(self);
        while let Some(entry) = current {
            for value in entry.0.bindings.values() {
                visit(*value);
            }
            current = entry.0.parent.as_ref();
        }
    }

    pub(crate) fn extend(&self, name: Symbol, value: RootedPtr) -> Self {
        let mut bindings = BTreeMap::new();
        bindings.insert(name, value);
        Self(Arc::new(ScopedEnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn extend_many(&self, bindings: BTreeMap<Symbol, RootedPtr>) -> Self {
        Self(Arc::new(ScopedEnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn get(&self, name: &Symbol) -> Option<RootedPtr> {
        let mut current = Some(self);
        while let Some(env) = current {
            if let Some(value) = env.0.bindings.get(name) {
                return Some(*value);
            }
            current = env.0.parent.as_ref();
        }
        None
    }

    pub(crate) fn from_entries(entries: Vec<BTreeMap<Symbol, RootedPtr>>) -> Self {
        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(Self(Arc::new(ScopedEnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }
        rebuilt.unwrap_or_else(Self::new)
    }
}

struct RootedEnvEntry {
    parent: Option<RootedEnvironment>,
    bindings: BTreeMap<Symbol, RootedPtr>,
}

/// Long-lived immutable environment backed by stable runtime root tokens.
///
/// Compiler output and runtime registries use this representation between
/// synchronous evaluator cycles. Creating a [`ScopedEnvironment`] copies only
/// the stable tokens; no heap lookup or persistence transformation is needed.
#[derive(Clone)]
pub(crate) struct RootedEnvironment(Arc<RootedEnvEntry>);

impl RootedEnvironment {
    pub(crate) fn new() -> Self {
        RootedEnvironment(Arc::new(RootedEnvEntry {
            parent: None,
            bindings: BTreeMap::new(),
        }))
    }

    pub(crate) fn extend(&self, name: Symbol, value: RootedPtr) -> Self {
        let mut bindings = BTreeMap::new();
        bindings.insert(name, value);
        RootedEnvironment(Arc::new(RootedEnvEntry {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub(crate) fn get(&self, name: &Symbol) -> Option<RootedPtr> {
        let mut current: Option<&RootedEnvironment> = Some(self);
        while let Some(env) = current {
            if let Some(v) = env.0.bindings.get(name) {
                return Some(*v);
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

    pub(crate) fn visit_values(&self, visit: &mut impl FnMut(RootedPtr)) {
        let mut current = Some(self);
        while let Some(env) = current {
            for value in env.0.bindings.values() {
                visit(*value);
            }
            current = env.0.parent.as_ref();
        }
    }

    pub(crate) fn to_scoped_environment(&self) -> ScopedEnvironment {
        let mut entries = Vec::new();
        let mut current = Some(self);
        while let Some(env) = current {
            let mut bindings = BTreeMap::new();
            for (name, root) in &env.0.bindings {
                bindings.insert(name.clone(), *root);
            }
            entries.push(bindings);
            current = env.0.parent.as_ref();
        }

        ScopedEnvironment::from_entries(entries)
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
    use crate::{EngineError, memory::heap::Heap};

    #[test]
    fn rooted_environment_survives_copying_gc() {
        let mut heap = Heap::new();
        let (a, b) = heap
            .machine_root_scope(|scope| {
                Ok::<_, EngineError>((scope.alloc_root_i32(1)?, scope.alloc_root_i32(2)?))
            })
            .unwrap();

        let rooted = RootedEnvironment::new()
            .extend(Symbol::intern("a"), a)
            .extend(Symbol::intern("b"), b);

        heap.collect().unwrap();
        heap.machine_root_scope(|scope| {
            let env = rooted.to_scoped_environment();
            scope.alloc_root_i32(3)?;
            let a = env.get(&Symbol::intern("a")).unwrap();
            let b = env.get(&Symbol::intern("b")).unwrap();
            assert_eq!(scope.root_as_i32(a)?, 1);
            assert_eq!(scope.root_as_i32(b)?, 2);
            Ok::<_, EngineError>(())
        })
        .unwrap();
    }
}
