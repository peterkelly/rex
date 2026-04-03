use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use rexlang_ast::expr::{Symbol, intern};
use rexlang_typesystem::Type;

use crate::Pointer;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LibraryId {
    Local { path: PathBuf },
    Remote(String),
    Virtual(String),
}

impl fmt::Display for LibraryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LibraryId::Local { path } => write!(f, "file:{}", path.display()),
            LibraryId::Remote(url) => write!(f, "{url}"),
            LibraryId::Virtual(name) => write!(f, "virtual:{name}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolveRequest {
    pub library_name: String,
    pub importer: Option<LibraryId>,
}

#[derive(Clone, Debug)]
pub struct ResolvedLibrary {
    pub id: LibraryId,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LibraryKey(u64);

impl LibraryKey {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Value,
    Type,
    Class,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CanonicalSymbol {
    pub library: LibraryKey,
    pub kind: SymbolKind,
    pub local: Symbol,
    symbol: Symbol,
}

impl CanonicalSymbol {
    pub fn new(library: LibraryKey, kind: SymbolKind, local: Symbol) -> Self {
        let symbol = intern(&format!(
            "{}.{}",
            prefix_for_library_key(library),
            local.as_ref()
        ));
        Self {
            library,
            kind,
            local,
            symbol,
        }
    }

    pub fn from_symbol(
        library: LibraryKey,
        kind: SymbolKind,
        local: Symbol,
        symbol: Symbol,
    ) -> Self {
        Self {
            library,
            kind,
            local,
            symbol,
        }
    }

    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }
}

#[derive(Clone)]
pub struct LibraryExports {
    pub values: HashMap<Symbol, CanonicalSymbol>,
    pub types: HashMap<Symbol, CanonicalSymbol>,
    pub classes: HashMap<Symbol, CanonicalSymbol>,
}

#[derive(Clone)]
pub struct VirtualLibraryModule {
    pub exports: LibraryExports,
    pub decls: Vec<rexlang_ast::expr::Decl>,
    pub source: Option<String>,
}

#[derive(Clone, Default)]
pub struct ReplState {
    pub(crate) alias_exports: HashMap<Symbol, LibraryExports>,
    pub(crate) imported_values: HashMap<Symbol, CanonicalSymbol>,
    pub(crate) defined_values: HashSet<Symbol>,
    pub(crate) importer_path: Option<PathBuf>,
}

impl ReplState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_importer_path(path: impl AsRef<Path>) -> Self {
        Self {
            importer_path: Some(path.as_ref().to_path_buf()),
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub struct LibraryInstance {
    pub id: LibraryId,
    pub exports: LibraryExports,
    pub init_value: Pointer,
    pub init_type: Type,
    pub source_fingerprint: Option<String>,
}

pub(crate) fn library_key_for_library(id: &LibraryId) -> LibraryKey {
    // Use a stable hash over stable identity bytes so canonical internal symbols
    // are deterministic across process runs/toolchains.
    // FNV-1a reference:
    // - Fowler, Noll, Vo hash function (public domain), 64-bit variant.
    let mut hash: u64 = 0xcbf29ce484222325;
    hash_library_identity(&mut hash, id);
    LibraryKey(hash)
}

fn hash_library_identity(state: &mut u64, id: &LibraryId) {
    fn hash_bytes(state: &mut u64, bytes: &[u8]) {
        for b in bytes {
            *state ^= u64::from(*b);
            *state = state.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    match id {
        LibraryId::Local { path } => {
            hash_bytes(state, b"local:");
            hash_bytes(state, path.as_os_str().as_encoded_bytes());
        }
        LibraryId::Remote(url) => {
            hash_bytes(state, b"remote:");
            hash_bytes(state, url.as_bytes());
        }
        LibraryId::Virtual(name) => {
            hash_bytes(state, b"virtual:");
            hash_bytes(state, name.as_bytes());
        }
    }
}

pub(crate) fn prefix_for_library_key(key: LibraryKey) -> String {
    format!("@m{:016x}", key.as_u64())
}

pub(crate) fn prefix_for_library(id: &LibraryId) -> String {
    prefix_for_library_key(library_key_for_library(id))
}

pub(crate) fn qualify(prefix: &str, name: &Symbol) -> Symbol {
    intern(&format!("{prefix}.{}", name.as_ref()))
}

pub fn virtual_export_name(library: &str, export: &str) -> String {
    let id = LibraryId::Virtual(library.to_string());
    let key = library_key_for_library(&id);
    CanonicalSymbol::new(key, SymbolKind::Value, intern(export))
        .symbol()
        .to_string()
}
