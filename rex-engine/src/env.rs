use std::collections::HashMap;
use std::sync::Arc;

use rex_ast::expr::Symbol;

use crate::value::Pointer;

#[derive(Clone, Debug, PartialEq)]
pub struct Env(Arc<EnvFrame>);

#[derive(Default, Debug, PartialEq)]
struct EnvFrame {
    parent: Option<Env>,
    bindings: HashMap<Symbol, Pointer>,
}

impl Env {
    pub fn new() -> Self {
        Env(Arc::new(EnvFrame::default()))
    }

    pub fn extend(&self, name: Symbol, value: Pointer) -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(name, value);
        Env(Arc::new(EnvFrame {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub fn extend_many(&self, bindings: HashMap<Symbol, Pointer>) -> Self {
        Env(Arc::new(EnvFrame {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub fn get(&self, name: &Symbol) -> Option<Pointer> {
        let mut current: Option<&Env> = Some(self);
        while let Some(env) = current {
            if let Some(v) = env.0.bindings.get(name) {
                return Some(v.clone());
            }
            current = env.0.parent.as_ref();
        }
        None
    }

    pub(crate) fn parent(&self) -> Option<&Env> {
        self.0.parent.as_ref()
    }

    pub(crate) fn bindings(&self) -> &HashMap<Symbol, Pointer> {
        &self.0.bindings
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}
