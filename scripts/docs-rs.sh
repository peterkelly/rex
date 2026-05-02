#!/bin/bash

# Generate docs in the same format as on docs.rs, which includes the full
# contents of public exports from the rex module.

set -eu
cargo +nightly docs-rs -p rex --open
