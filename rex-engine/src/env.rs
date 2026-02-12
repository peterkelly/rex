use std::collections::HashMap;
use std::sync::Arc;

use rex_ast::expr::Symbol;

use crate::value::Pointer;

#[derive(Clone, Debug, PartialEq)]
pub struct Env<'h>(Arc<EnvFrame<'h>>);

#[derive(Default, Debug, PartialEq)]
struct EnvFrame<'h> {
    parent: Option<Env<'h>>,
    bindings: HashMap<Symbol, Pointer<'h>>,
}

impl<'h> Env<'h> {
    pub fn new() -> Self {
        Env(Arc::new(EnvFrame::default()))
    }

    pub fn extend(&self, name: Symbol, value: Pointer<'h>) -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(name, value);
        Env(Arc::new(EnvFrame {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub fn extend_many(&self, bindings: HashMap<Symbol, Pointer<'h>>) -> Self {
        Env(Arc::new(EnvFrame {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub fn get(&self, name: &Symbol) -> Option<Pointer<'h>> {
        let mut current: Option<&Env<'h>> = Some(self);
        while let Some(env) = current {
            if let Some(v) = env.0.bindings.get(name) {
                return Some(v.clone());
            }
            current = env.0.parent.as_ref();
        }
        None
    }
}

impl<'h> Default for Env<'h> {
    fn default() -> Self {
        Self::new()
    }
}
