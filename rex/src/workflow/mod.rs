//! Workflow execution, installable tool discovery, and OCI-backed tool support.

// This module predates the crate-level missing-documentation lint. Keep the
// merge behavior-neutral; its public API documentation can be improved
// incrementally without blocking consolidation of the two crates.
#![allow(missing_docs)]

pub mod config;
pub mod executor;
pub mod run;
pub mod state;
pub mod tool_protocol;
pub mod tools;
