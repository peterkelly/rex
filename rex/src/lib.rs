#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Rex CLI crate.
//!
//! This library is intentionally empty today: the primary entry point is the
//! binary in `rex/src/main.rs`. Keeping `lib.rs` around makes it easy to grow a
//! reusable API later without reorganizing the crate layout.
