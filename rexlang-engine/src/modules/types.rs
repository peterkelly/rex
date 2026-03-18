use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use rexlang_ast::expr::{Symbol, intern};
use rexlang_ts::Type;

use crate::Pointer;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleId {
    Local { path: PathBuf },
    Remote(String),
    Virtual(String),
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleId::Local { path } => write!(f, "file:{}", path.display()),
            ModuleId::Remote(url) => write!(f, "{url}"),
            ModuleId::Virtual(name) => write!(f, "virtual:{name}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolveRequest {
    pub module_name: String,
    pub importer: Option<ModuleId>,
}

#[derive(Clone, Debug)]
pub struct ResolvedModule {
    pub id: ModuleId,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModuleKey(u64);

impl ModuleKey {
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
    pub module: ModuleKey,
    pub kind: SymbolKind,
    pub local: Symbol,
    symbol: Symbol,
}

impl CanonicalSymbol {
    pub fn new(module: ModuleKey, kind: SymbolKind, local: Symbol) -> Self {
        let symbol = intern(&format!(
            "{}.{}",
            prefix_for_module_key(module),
            local.as_ref()
        ));
        Self {
            module,
            kind,
            local,
            symbol,
        }
    }

    pub fn from_symbol(module: ModuleKey, kind: SymbolKind, local: Symbol, symbol: Symbol) -> Self {
        Self {
            module,
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
pub struct ModuleExports {
    pub values: HashMap<Symbol, CanonicalSymbol>,
    pub types: HashMap<Symbol, CanonicalSymbol>,
    pub classes: HashMap<Symbol, CanonicalSymbol>,
}

#[derive(Clone, Default)]
pub struct ReplState {
    pub(crate) alias_exports: HashMap<Symbol, ModuleExports>,
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
pub struct ModuleInstance {
    pub id: ModuleId,
    pub exports: ModuleExports,
    pub init_value: Pointer,
    pub init_type: Type,
    pub source_fingerprint: Option<String>,
}

pub(crate) fn module_key_for_module(id: &ModuleId) -> ModuleKey {
    // Use a stable hash over stable identity bytes so canonical internal symbols
    // are deterministic across process runs/toolchains.
    // FNV-1a reference:
    // - Fowler, Noll, Vo hash function (public domain), 64-bit variant.
    let mut hash: u64 = 0xcbf29ce484222325;
    hash_module_identity(&mut hash, id);
    ModuleKey(hash)
}

fn hash_module_identity(state: &mut u64, id: &ModuleId) {
    fn hash_bytes(state: &mut u64, bytes: &[u8]) {
        for b in bytes {
            *state ^= u64::from(*b);
            *state = state.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    match id {
        ModuleId::Local { path } => {
            hash_bytes(state, b"local:");
            hash_bytes(state, path.as_os_str().as_encoded_bytes());
        }
        ModuleId::Remote(url) => {
            hash_bytes(state, b"remote:");
            hash_bytes(state, url.as_bytes());
        }
        ModuleId::Virtual(name) => {
            hash_bytes(state, b"virtual:");
            hash_bytes(state, name.as_bytes());
        }
    }
}

pub(crate) fn prefix_for_module_key(key: ModuleKey) -> String {
    format!("@m{:016x}", key.as_u64())
}

pub(crate) fn prefix_for_module(id: &ModuleId) -> String {
    prefix_for_module_key(module_key_for_module(id))
}

pub(crate) fn qualify(prefix: &str, name: &Symbol) -> Symbol {
    intern(&format!("{prefix}.{}", name.as_ref()))
}

pub fn virtual_export_name(module: &str, export: &str) -> String {
    let id = ModuleId::Virtual(module.to_string());
    let key = module_key_for_module(&id);
    CanonicalSymbol::new(key, SymbolKind::Value, intern(export))
        .symbol()
        .to_string()
}
