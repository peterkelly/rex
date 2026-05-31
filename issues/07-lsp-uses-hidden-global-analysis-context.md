# LSP Uses Hidden Global Analysis Context

## Problem

The LSP analysis layer relies on global and thread-local state for parse caching and open-document snapshots. This makes analysis functions depend on ambient context rather than explicit inputs.

## Evidence

- `rex-lsp/src/shared.rs` defines a global parse cache using `OnceLock<Mutex<HashMap<Url, CachedParse>>>`.
- `rex-lsp/src/shared.rs` defines thread-local `OPEN_DOCUMENTS`.
- `with_open_documents()` installs an open-document snapshot into thread-local state while a closure runs.
- `rex-lsp/src/tower.rs` clones the open document map and enters that context inside `tokio::task::spawn_blocking`.

## Why This Smells

The LSP has to combine file-system state, open unsaved buffers, parse caches, and semantic analysis. That is normal. The smell is that this state is hidden from the signatures of many analysis functions.

Ambient state creates several problems:

- It is harder to test analysis functions in isolation.
- Concurrency assumptions are implicit.
- Future multi-workspace behavior becomes riskier.
- WASM and native LSP modes may accidentally differ.
- Cache invalidation is spread across call sites rather than owned by an explicit session object.

The use of `spawn_blocking` with a thread-local document snapshot is particularly subtle: correctness depends on entering the context in the exact thread where analysis runs.

## Impact

This is mostly a maintainability and testability issue today. It can become a correctness issue if LSP analysis becomes more concurrent, supports multiple workspaces, or shares more logic with browser/WASM tooling.

An explicit analysis/session object would make dependencies clearer, but this issue should first be treated as a problem definition: too much LSP semantic behavior depends on hidden context.

