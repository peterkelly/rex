#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod json;

pub use rex_ast::expr::{Program, intern, sym};
pub use rex_engine::{
    AsyncHandler, Engine, EngineError, Export, FromPointer, Handler, Heap, IntoPointer, Module,
    Pointer, RexType, Value, closure_debug, closure_eq, value_debug, value_display, value_eq,
};
pub use rex_lexer::Token;
pub use rex_parser::{Parser, ParserLimits};
pub use rex_proc_macro::Rex;
pub use rex_ts::{AdtDecl, Type, TypeError, TypeSystem};
pub use rex_util::{GasCosts, GasMeter};

pub use crate::json::{EnumPatch, JsonOptions, json_to_rex, rex_to_json};
