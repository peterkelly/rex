#!/bin/bash
set -euo pipefail

cleanup_artifacts() {
  cargo clean
  rm -rf docs/book docs/src/assets/rex-wasm docs/_build docs/_static docs/_templates
}

cleanup_artifacts
trap cleanup_artifacts EXIT

cargo fmt --all -- --check
cargo build
cargo check --tests
cargo clippy --tests -- -D warnings
cargo test

cargo run -p rex --bin gen_prelude_docs
git diff --exit-code docs/src/PRELUDE.md

mdbook build docs
cp llms.txt docs/book/llms.txt
test -f docs/book/index.html
test -f docs/book/llms.txt
