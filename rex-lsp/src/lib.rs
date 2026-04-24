#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod server;

#[cfg(not(target_arch = "wasm32"))]
pub mod tower;
