#![forbid(unsafe_code)]

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
pub use error::EngineError;
pub use modules::{ModuleExports, ModuleId, ModuleInstance, ReplState, ResolveRequest, ResolvedModule};
pub use modules::virtual_export_name;
pub use stack::DEFAULT_STACK_SIZE_BYTES;
