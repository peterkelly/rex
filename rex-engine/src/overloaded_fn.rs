use crate::memory::{
    heap::{Pointer, RootScope, RootedPtr},
    traits::Collection,
};
use rex_ast::Symbol;
use rex_typesystem::types::Type;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OverloadedFn<P> {
    pub(crate) name: Symbol,
    pub(crate) typ: Type,
    pub(crate) applied: Vec<P>,
    pub(crate) applied_types: Vec<Type>,
}

impl OverloadedFn<Pointer> {
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

    pub(crate) fn rooted<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> OverloadedFn<RootedPtr<'scope>> {
        OverloadedFn {
            name: self.name.clone(),
            typ: self.typ.clone(),
            applied: self
                .applied
                .iter()
                .map(|value| scope.root(*value))
                .collect(),
            applied_types: self.applied_types.clone(),
        }
    }

    pub(crate) fn into_parts(self) -> (Symbol, Type, Vec<Pointer>, Vec<Type>) {
        (self.name, self.typ, self.applied, self.applied_types)
    }
}

impl<P> OverloadedFn<P> {
    pub(crate) fn name(&self) -> &Symbol {
        &self.name
    }
}

impl Collection for OverloadedFn<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        for pointer in &mut self.applied {
            *pointer = map(*pointer)?;
        }
        Ok(())
    }
}
