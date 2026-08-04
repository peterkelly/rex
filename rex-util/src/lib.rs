#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod imports;

pub use imports::{ImportPathError, resolve_local_import_path};
