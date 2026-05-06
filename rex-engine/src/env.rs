use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use rex_ast::expr::Symbol;

use crate::EngineError;
use crate::value::Pointer;

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

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        let mut current = Some(self);
        while let Some(env) = current {
            out.extend(env.0.bindings.values().copied());
            current = env.0.parent.as_ref();
        }
    }

    pub(crate) fn trace_pointers_shared(&self, seen: &mut HashSet<usize>, out: &mut Vec<Pointer>) {
        let mut current = Some(self);
        while let Some(env) = current {
            if !seen.insert(env.entry_id()) {
                break;
            }
            out.extend(env.0.bindings.values().copied());
            current = env.0.parent.as_ref();
        }
    }

    pub(crate) fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        let mut entries = Vec::new();
        let mut current: Option<&Environment> = Some(self);
        while let Some(env) = current {
            let mut bindings = BTreeMap::new();
            for (name, pointer) in &env.0.bindings {
                bindings.insert(name.clone(), rewrite(*pointer)?);
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

    pub(crate) fn rewrite_pointers_shared(
        &mut self,
        rewritten: &mut HashMap<usize, Environment>,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        let mut entries = Vec::new();
        let mut current: Option<&Environment> = Some(self);
        let mut rebuilt = None;
        while let Some(env) = current {
            let id = env.entry_id();
            if let Some(existing) = rewritten.get(&id) {
                rebuilt = Some(existing.clone());
                break;
            }

            let mut bindings = BTreeMap::new();
            for (name, pointer) in &env.0.bindings {
                bindings.insert(name.clone(), rewrite(*pointer)?);
            }
            entries.push((id, bindings));
            current = env.0.parent.as_ref();
        }

        for (id, bindings) in entries.into_iter().rev() {
            let env = Environment(Arc::new(EnvEntry {
                parent: rebuilt,
                bindings,
            }));
            rewritten.insert(id, env.clone());
            rebuilt = Some(env);
        }

        *self = rebuilt.unwrap_or_default();
        Ok(())
    }

    fn entry_id(&self) -> usize {
        // EnvEntry arcs are immutable, so identity is enough within one rewrite pass.
        Arc::as_ptr(&self.0) as usize
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Heap;

    #[test]
    fn shared_pointer_rewrite_visits_shared_entries_once() {
        let heap = Heap::new();
        let a = heap.alloc_ptr_i32(1).unwrap();
        let b = heap.alloc_ptr_i32(2).unwrap();
        let c = heap.alloc_ptr_i32(3).unwrap();

        let base = Environment::new().extend(Symbol::intern("a"), a);
        let mut left = base.extend(Symbol::intern("b"), b);
        let mut right = base.extend(Symbol::intern("c"), c);

        let mut traced = Vec::new();
        let mut seen = HashSet::new();
        left.trace_pointers_shared(&mut seen, &mut traced);
        right.trace_pointers_shared(&mut seen, &mut traced);
        assert_eq!(traced.len(), 3);

        let mut rewritten = HashMap::new();
        let mut rewrite_count = 0;
        let mut rewrite = |pointer| {
            rewrite_count += 1;
            Ok(pointer)
        };
        left.rewrite_pointers_shared(&mut rewritten, &mut rewrite)
            .unwrap();
        right
            .rewrite_pointers_shared(&mut rewritten, &mut rewrite)
            .unwrap();
        assert_eq!(rewrite_count, 3);

        let left_parent = left.0.parent.as_ref().unwrap();
        let right_parent = right.0.parent.as_ref().unwrap();
        assert!(Arc::ptr_eq(&left_parent.0, &right_parent.0));
    }
}
