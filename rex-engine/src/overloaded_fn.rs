use rex_ast::Symbol;
use rex_typesystem::types::Type;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OverloadedFn<P> {
    pub(crate) name: Symbol,
    pub(crate) typ: Type,
    pub(crate) applied: Vec<P>,
    pub(crate) applied_types: Vec<Type>,
}

impl<P> OverloadedFn<P> {
    pub(crate) fn from_parts(
        name: Symbol,
        typ: Type,
        applied: Vec<P>,
        applied_types: Vec<Type>,
    ) -> Self {
        Self {
            name,
            typ,
            applied,
            applied_types,
        }
    }
}
