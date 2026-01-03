#![forbid(unsafe_code)]

//! Evaluation engine for Rex.

mod engine;
mod env;
mod error;
mod cancel;
mod stack;

pub use cancel::CancellationToken;
pub use engine::{Engine, FromValue, IntoValue, NativeFn, OverloadedFn, RexType, Value};
pub use env::Env;
pub use error::EngineError;
pub use stack::DEFAULT_STACK_SIZE_BYTES;
