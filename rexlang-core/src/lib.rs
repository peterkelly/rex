#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod json;

pub use rexlang_ast::expr::{Program, intern, sym};
pub use rexlang_engine::{
    AsyncHandler, AsyncNativeCallable, AsyncNativeCallableCancellable, Engine, EngineError,
    EngineOptions, Export, FromPointer, Handler, Heap, IntoPointer, Library, NativeFuture,
    PRELUDE_LIBRARY_NAME, Pointer, PreludeMode, ROOT_LIBRARY_NAME, RexAdt, RexDefault, RexType,
    SyncNativeCallable, Value, ValueDisplayOptions, assert_pointer_eq, closure_debug, closure_eq,
    collect_adts_error_to_engine, pointer_display, pointer_display_with, value_debug, value_eq,
};
pub use rexlang_lexer::Token;
pub use rexlang_parser::{Parser, ParserLimits, error::ParserErr};
pub use rexlang_proc_macro::Rex;
pub use rexlang_typesystem::{
    AdtConflict, AdtDecl, BuiltinTypeId, CollectAdtsError, Scheme, Type, TypeError, TypeKind,
    TypeSystem, collect_adts_in_types,
};
pub use rexlang_util::{GasCosts, GasMeter};

pub use crate::json::{EnumPatch, JsonOptions, json_to_rex, rex_to_json};
