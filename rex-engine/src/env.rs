use std::collections::BTreeMap;
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
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}
