#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Evaluation engine for Rex.

mod cancel;
mod engine;
mod env;
mod error;
mod modules;
mod stack;

pub use cancel::CancellationToken;
pub use engine::{Engine, FromValue, IntoValue, NativeFn, OverloadedFn, RexType, Value};
pub use env::Env;
pub use error::{EngineError, ModuleError};
pub use modules::virtual_export_name;
pub use modules::{
    ModuleExports, ModuleId, ModuleInstance, ReplState, ResolveRequest, ResolvedModule,
};
pub use stack::DEFAULT_STACK_SIZE_BYTES;
