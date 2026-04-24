#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

#[cfg_attr(not(target_arch = "wasm32"), tokio::main)]
#[cfg(not(target_arch = "wasm32"))]
async fn main() {
    rex_lsp::tower::run_stdio().await;
}

#[cfg(target_arch = "wasm32")]
fn main() {}
