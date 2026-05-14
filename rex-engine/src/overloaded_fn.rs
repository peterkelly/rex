use crate::{
    error::EngineError,
    value::{Collection, Pointer},
};
use rex_ast::Symbol;
use rex_typesystem::types::Type;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OverloadedFn {
    pub(crate) name: Symbol,
    pub(crate) typ: Type,
    pub(crate) applied: Vec<Pointer>,
    pub(crate) applied_types: Vec<Type>,
}

impl OverloadedFn {
    pub(crate) fn new(name: Symbol, typ: Type) -> Self {
        Self {
            name,
            typ,
            applied: Vec::new(),
            applied_types: Vec::new(),
        }
    }

    pub(crate) fn from_parts(
        name: Symbol,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Self {
        Self {
            name,
            typ,
            applied,
            applied_types,
        }
    }

    pub(crate) fn name(&self) -> &Symbol {
        &self.name
    }

    pub(crate) fn into_parts(self) -> (Symbol, Type, Vec<Pointer>, Vec<Type>) {
        (self.name, self.typ, self.applied, self.applied_types)
    }
}

impl Collection for OverloadedFn {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.extend(self.applied.iter().copied());
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        for pointer in &mut self.applied {
            *pointer = rewrite(*pointer)?;
        }
        Ok(())
    }
}
