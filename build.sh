#!/bin/bash
set -euo pipefail
STEPS=11

cleanup_artifacts() {
  cargo clean
  rm -rf docs/book docs/src/assets/rex-wasm docs/_build docs/_static docs/_templates
}

cleanup_artifacts
trap cleanup_artifacts EXIT

echo build.sh Step 1 of $STEPS: cargo fmt --all -- --check
cargo fmt --all -- --check

echo build.sh Step 2 of $STEPS: cargo build
cargo build

echo build.sh Step 3 of $STEPS: cargo check --tests
cargo check --tests

echo build.sh Step 4 of $STEPS: cargo clippy --tests -- -D warnings
cargo clippy --tests -- -D warnings

echo build.sh Step 5 of $STEPS: cargo test
cargo test

echo build.sh Step 6 of $STEPS: cargo run -p rex-cli --bin gen_prelude_docs
cargo run -p rex-cli --bin gen_prelude_docs

echo build.sh Step 7 of $STEPS: git diff --exit-code docs/src/PRELUDE.md
git diff --exit-code docs/src/PRELUDE.md

echo build.sh Step 8 of $STEPS: mdbook build docs
mdbook build docs

echo build.sh Step 9 of $STEPS: cp llms.txt docs/book/llms.txt
cp llms.txt docs/book/llms.txt

echo build.sh Step 10 of $STEPS: test -f docs/book/index.html
test -f docs/book/index.html

echo build.sh Step 11 of $STEPS: test -f docs/book/llms.txt
test -f docs/book/llms.txt

echo build.sh Completed successfully
