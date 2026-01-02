use std::collections::HashMap;
use std::sync::Arc;

use rex_ast::expr::Symbol;

use crate::Value;

#[derive(Clone)]
pub struct Env(Arc<EnvFrame>);

#[derive(Default)]
struct EnvFrame {
    parent: Option<Env>,
    bindings: HashMap<Symbol, Value>,
}

impl Env {
    pub fn new() -> Self {
        Env(Arc::new(EnvFrame::default()))
    }

    pub fn extend(&self, name: Symbol, value: Value) -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(name, value);
        Env(Arc::new(EnvFrame {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub fn extend_many(&self, bindings: HashMap<Symbol, Value>) -> Self {
        Env(Arc::new(EnvFrame {
            parent: Some(self.clone()),
            bindings,
        }))
    }

    pub fn get(&self, name: &Symbol) -> Option<Value> {
        let mut current: Option<&Env> = Some(self);
        while let Some(env) = current {
            if let Some(v) = env.0.bindings.get(name) {
                return Some(v.clone());
            }
            current = env.0.parent.as_ref();
        }
        None
    }
}

