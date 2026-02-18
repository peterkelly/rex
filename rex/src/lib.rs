#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod json;

pub use rex_ast::expr::{Program, intern, sym};
pub use rex_engine::{
    AsyncHandler, AsyncNativeCallable, AsyncNativeCallableCancellable, Engine, EngineError, Export,
    FromPointer, Handler, Heap, IntoPointer, Module, NativeFuture, Pointer, RexAdt, RexType,
    SyncNativeCallable, Value, ValueDisplayOptions, closure_debug, closure_eq, pointer_display,
    pointer_display_with, value_debug, value_eq,
};
pub use rex_lexer::Token;
pub use rex_parser::{Parser, ParserLimits, error::ParserErr};
pub use rex_proc_macro::Rex;
pub use rex_ts::{AdtDecl, Scheme, Type, TypeError, TypeKind, TypeSystem};
pub use rex_util::{GasCosts, GasMeter};

pub use crate::json::{EnumPatch, JsonOptions, json_to_rex, rex_to_json};
