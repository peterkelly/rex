use crate::{env::Environment, manifest::MainSignature};
use rex_ast::Symbol;
use rex_typesystem::types::{Type, TypedExpr};
use std::sync::Arc;

/// Prepared Rex code plus the environment snapshot needed to run it.
pub struct CompiledProgram {
    /// Name-level summary of external runtime bindings referenced by this program.
    externs: CompiledExterns,
    main_signature: MainSignature,
    pub(crate) env: Environment,
    pub(crate) expr: Arc<TypedExpr>,
}

impl CompiledProgram {
    pub(crate) fn new(
        externs: CompiledExterns,
        main_signature: MainSignature,
        env: Environment,
        expr: TypedExpr,
    ) -> Self {
        Self {
            externs,
            main_signature,
            env,
            expr: Arc::new(expr),
        }
    }

    /// Externally visible result type after applying all main inputs.
    pub fn result_type(&self) -> &Type {
        self.main_signature.result_type()
    }

    /// Externally visible main input and result types.
    pub fn main_signature(&self) -> &MainSignature {
        &self.main_signature
    }

    /// Name-level summary of external native and class-method references.
    pub fn externs(&self) -> &CompiledExterns {
        &self.externs
    }
}

/// Name-level summary of external bindings referenced by compiled code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompiledExterns {
    /// Native function names referenced by the prepared expression.
    pub natives: Vec<Symbol>,
    /// Typeclass method names referenced by the prepared expression.
    pub class_methods: Vec<Symbol>,
}

impl CompiledExterns {
    /// Return true when no external bindings were referenced.
    pub fn is_empty(&self) -> bool {
        self.natives.is_empty() && self.class_methods.is_empty()
    }
}
