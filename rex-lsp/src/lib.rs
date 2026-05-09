#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub const CMD_EXPECTED_TYPE_AT: &str = "rex.expectedTypeAt";
pub const CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT: &str = "rex.functionsProducingExpectedTypeAt";
pub const CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT: &str = "rex.functionsAcceptingInferredTypeAt";
pub const CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT: &str = "rex.adaptersFromInferredToExpectedAt";
pub const CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT: &str =
    "rex.functionsCompatibleWithInScopeValuesAt";
pub const CMD_HOLES_EXPECTED_TYPES: &str = "rex.holesExpectedTypes";
pub const CMD_SEMANTIC_LOOP_STEP: &str = "rex.semanticLoopStep";
pub const CMD_SEMANTIC_LOOP_APPLY_QUICK_FIX_AT: &str = "rex.semanticLoopApplyQuickFixAt";
pub const CMD_SEMANTIC_LOOP_APPLY_BEST_QUICK_FIXES_AT: &str =
    "rex.semanticLoopApplyBestQuickFixesAt";
pub(crate) const MAX_DIAGNOSTICS: usize = 50;
pub(crate) const NO_IMPROVEMENT_STREAK_LIMIT: usize = 2;
pub const MAX_SEMANTIC_ENV_SCHEMES_SCAN: usize = 1024;
pub const MAX_SEMANTIC_IN_SCOPE_VALUES: usize = 128;
pub const MAX_SEMANTIC_CANDIDATES: usize = 64;
pub const MAX_SEMANTIC_HOLE_FILL_ARITY: usize = 8;
pub const MAX_SEMANTIC_HOLES: usize = 128;
pub(crate) const BUILTIN_TYPES: &[&str] = &[
    "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "string", "uuid",
    "datetime", "Dict", "List", "Array", "Option", "Result",
];
pub(crate) const BUILTIN_VALUES: &[&str] = &["true", "false", "null", "Some", "None", "Ok", "Err"];

pub mod code_actions;
pub mod completion;
pub mod diagnostics;
pub mod document;
pub mod imports;
pub mod navigation;
mod prelude;
pub mod public;
pub mod queries;
pub mod shared;

#[cfg(not(target_arch = "wasm32"))]
pub mod tower;
