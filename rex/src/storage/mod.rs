//! Content-addressed storage for immutable blobs and directory trees.
//!
//! [`Store`](crate::storage::Store) provides in-memory, filesystem,
//! object-store, and custom backends. [`import_path`](crate::storage::import_path),
//! [`export_tree`](crate::storage::export_tree), and
//! [`export_blob`](crate::storage::export_blob) transfer stored content across
//! the local filesystem boundary.

mod entry;
mod filesystem;
mod memory;
mod object_store;
mod store;
mod transfer;

pub use entry::{Entry, EntryKind};
pub use store::{Store, StoreError, StoreFuture, StoreImpl};
pub use transfer::{export_blob, export_tree, import_path};
