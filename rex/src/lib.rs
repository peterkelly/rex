#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub use rex_ast::expr::{Program, intern, sym};
pub use rex_engine::{Engine, EngineError, FromValue, IntoValue, RexType, Value};
pub use rex_lexer::Token;
pub use rex_parser::Parser;
pub use rex_proc_macro::Rex;
pub use rex_ts::{AdtDecl, Type};
pub use rex_util::{GasCosts, GasMeter};
