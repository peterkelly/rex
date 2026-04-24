use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use rex_ast::expr::{Program, Symbol, intern};
use rex_typesystem::types::Type;

use crate::Pointer;

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ResolveRequest {
    pub module_name: String,
    pub importer: Option<ModuleId>,
}

#[derive(Clone, Debug)]
pub enum ResolvedModuleContent {
    Source(String),
    Program(Program),
}

#[derive(Clone, Debug)]
pub struct ResolvedModule {
    pub id: ModuleId,
    pub content: ResolvedModuleContent,
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ModuleKey(u64);

impl ModuleKey {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum SymbolKind {
    Value,
    Type,
    Class,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct CanonicalSymbol {
    pub module: ModuleKey,
    pub kind: SymbolKind,
    pub local: Symbol,
    pub symbol: Symbol,
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

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ExportEntry {
    pub value: Option<CanonicalSymbol>,
    pub typ: Option<CanonicalSymbol>,
    pub class: Option<CanonicalSymbol>,
}

impl ExportEntry {
    pub fn new() -> Self {
        Self {
            value: None,
            typ: None,
            class: None,
        }
    }
}

impl Default for ExportEntry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct ModuleExports {
    pub entries: BTreeMap<Symbol, ExportEntry>,
}

impl ModuleExports {
    pub fn entry(&self, name: &Symbol) -> Option<&ExportEntry> {
        self.entries.get(name)
    }

    pub fn value(&self, name: &Symbol) -> Option<&CanonicalSymbol> {
        self.entry(name).and_then(|entry| entry.value.as_ref())
    }

    pub fn typ(&self, name: &Symbol) -> Option<&CanonicalSymbol> {
        self.entry(name).and_then(|entry| entry.typ.as_ref())
    }

    pub fn class(&self, name: &Symbol) -> Option<&CanonicalSymbol> {
        self.entry(name).and_then(|entry| entry.class.as_ref())
    }

    pub fn insert_value(&mut self, name: Symbol, symbol: CanonicalSymbol) {
        self.entries.entry(name).or_default().value = Some(symbol);
    }

    pub fn insert_type(&mut self, name: Symbol, symbol: CanonicalSymbol) {
        self.entries.entry(name).or_default().typ = Some(symbol);
    }

    pub fn insert_class(&mut self, name: Symbol, symbol: CanonicalSymbol) {
        self.entries.entry(name).or_default().class = Some(symbol);
    }

    pub fn values(&self) -> impl Iterator<Item = (&Symbol, &CanonicalSymbol)> {
        self.entries
            .iter()
            .filter_map(|(name, entry)| entry.value.as_ref().map(|symbol| (name, symbol)))
    }

    pub fn types(&self) -> impl Iterator<Item = (&Symbol, &CanonicalSymbol)> {
        self.entries
            .iter()
            .filter_map(|(name, entry)| entry.typ.as_ref().map(|symbol| (name, symbol)))
    }

    pub fn classes(&self) -> impl Iterator<Item = (&Symbol, &CanonicalSymbol)> {
        self.entries
            .iter()
            .filter_map(|(name, entry)| entry.class.as_ref().map(|symbol| (name, symbol)))
    }

    pub fn value_names(&self) -> Vec<Symbol> {
        self.values().map(|(name, _)| name.clone()).collect()
    }

    pub fn type_names(&self) -> Vec<Symbol> {
        self.types().map(|(name, _)| name.clone()).collect()
    }

    pub fn class_names(&self) -> Vec<Symbol> {
        self.classes().map(|(name, _)| name.clone()).collect()
    }
}

#[derive(Clone)]
pub struct VirtualModule {
    pub exports: ModuleExports,
    pub decls: Vec<rex_ast::expr::Decl>,
    pub source: Option<String>,
}

#[derive(Clone, Default)]
pub struct ReplState {
    pub(crate) alias_exports: BTreeMap<Symbol, ModuleExports>,
    pub(crate) imported_values: BTreeMap<Symbol, CanonicalSymbol>,
    pub(crate) imported_types: BTreeMap<Symbol, CanonicalSymbol>,
    pub(crate) imported_classes: BTreeMap<Symbol, CanonicalSymbol>,
    pub(crate) defined_values: BTreeSet<Symbol>,
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
