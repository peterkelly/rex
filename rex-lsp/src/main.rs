#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

#[tokio::main]
async fn main() {
    rex_lsp::run_stdio().await;
}
