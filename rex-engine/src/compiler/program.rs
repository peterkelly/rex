use crate::{env::RootedEnvironment, manifest::MainSignature};
use rex_typesystem::types::{Type, TypedExpr};
use std::sync::Arc;

/// Prepared Rex code plus the environment snapshot needed to run it.
pub struct CompiledProgram {
    main_signature: MainSignature,
    pub(crate) env: RootedEnvironment,
    pub(crate) expr: Arc<TypedExpr>,
}

impl CompiledProgram {
    pub(crate) fn new(
        main_signature: MainSignature,
        env: RootedEnvironment,
        expr: TypedExpr,
    ) -> Self {
        Self {
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
}
