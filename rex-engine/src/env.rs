use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rex_ast::Symbol;

use crate::EngineError;
use crate::value::{Handle, Heap, Pointer};

#[derive(Clone, Debug, PartialEq)]
pub struct Environment(Arc<EnvEntry>);

#[derive(Default, Debug, PartialEq)]
struct EnvEntry {
    parent: Option<Environment>,
    bindings: BTreeMap<Symbol, Pointer>,
}

#[derive(Clone)]
pub(crate) struct RootedEnvironment(Arc<RootedEnvEntry>);

struct RootedEnvEntry {
    parent: Option<RootedEnvironment>,
    bindings: BTreeMap<Symbol, Handle>,
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

    fn entry_id(&self) -> usize {
        // EnvEntry arcs are immutable, so identity is enough within one rewrite pass.
        Arc::as_ptr(&self.0) as usize
    }
}

impl RootedEnvironment {
    pub(crate) fn from_environment(env: &Environment, heap: &Heap) -> Result<Self, EngineError> {
        let mut rooted = HashMap::new();
        Self::from_environment_shared(env, heap, &mut rooted)
    }

    fn from_environment_shared(
        env: &Environment,
        heap: &Heap,
        rooted: &mut HashMap<usize, RootedEnvironment>,
    ) -> Result<Self, EngineError> {
        let id = env.entry_id();
        if let Some(existing) = rooted.get(&id) {
            return Ok(existing.clone());
        }

        let parent = env
            .0
            .parent
            .as_ref()
            .map(|parent| Self::from_environment_shared(parent, heap, rooted))
            .transpose()?;
        let mut bindings = BTreeMap::new();
        for (name, pointer) in &env.0.bindings {
            bindings.insert(name.clone(), heap.handle(*pointer)?);
        }

        let env = RootedEnvironment(Arc::new(RootedEnvEntry { parent, bindings }));
        rooted.insert(id, env.clone());
        Ok(env)
    }

    pub(crate) fn to_environment(&self) -> Result<Environment, EngineError> {
        let mut entries = Vec::new();
        let mut current = Some(self);
        while let Some(env) = current {
            let mut bindings = BTreeMap::new();
            for (name, handle) in &env.0.bindings {
                bindings.insert(name.clone(), handle.pointer()?);
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
    fn rooted_environment_survives_copying_gc() {
        let heap = Heap::new();
        let a = heap.alloc_ptr_i32(1).unwrap();
        let b = heap.alloc_ptr_i32(2).unwrap();

        let env = Environment::new()
            .extend(Symbol::intern("a"), a)
            .extend(Symbol::intern("b"), b);
        let rooted = RootedEnvironment::from_environment(&env, &heap).unwrap();

        heap.set_collect_on_every_alloc(true).unwrap();
        heap.alloc_ptr_i32(3).unwrap();

        let env = rooted.to_environment().unwrap();
        let a = env.get(&Symbol::intern("a")).unwrap();
        let b = env.get(&Symbol::intern("b")).unwrap();
        assert_eq!(heap.pointer_as_i32(&a).unwrap(), 1);
        assert_eq!(heap.pointer_as_i32(&b).unwrap(), 2);
    }
}
