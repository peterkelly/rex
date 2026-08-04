use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};

use crate::{EngineError, modules::ModuleId};
use rex_ast::Symbol;

use super::{CompilationPackage, Module};

/// Request passed from the module system to importers when Rex code imports a module.
///
/// It carries the requested module name plus the optional identity of the
/// importing module so importers can resolve relative names or enforce policy.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ImportRequest {
    pub module_id: ModuleId,
    pub importer: Option<ModuleId>,
}

impl ImportRequest {
    pub fn new(module_id: ModuleId) -> Self {
        Self {
            module_id,
            importer: None,
        }
    }

    pub fn with_importer(module_id: ModuleId, importer: ModuleId) -> Self {
        Self {
            module_id,
            importer: Some(importer),
        }
    }
}

/// Imported module payload returned by an [`Importer`](super::Importer).
///
/// Importers may return raw Rex source for the engine to parse or a prebuilt
/// compilation package when the caller has already parsed or synthesized the
/// AST. They may also return a Rust-backed module that will be installed lazily
/// by the compiler when the import is actually needed.
#[derive(Clone, Debug)]
pub enum ResolvedModuleContent<State: Clone + Send + Sync + 'static = ()> {
    /// Raw Rex source text.
    Source(String),
    /// Parsed or synthesized module declarations.
    CompilationPackage(CompilationPackage),
    /// Rust-backed host module to install into the engine.
    Module(ResolvedRustModule<State>),
}

impl<State> ResolvedModuleContent<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub fn module(module: Module<State>) -> Self {
        Self::Module(ResolvedRustModule::new(module))
    }
}

pub struct ResolvedRustModule<State: Clone + Send + Sync + 'static = ()> {
    module: Arc<Mutex<Option<Module<State>>>>,
}

impl<State> ResolvedRustModule<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub fn new(module: Module<State>) -> Self {
        Self {
            module: Arc::new(Mutex::new(Some(module))),
        }
    }

    pub(crate) fn take(&self) -> Result<Module<State>, EngineError> {
        let mut module = self
            .module
            .lock()
            .map_err(|_| EngineError::Internal("lazy Rust module lock was poisoned".to_string()))?;
        module
            .take()
            .ok_or_else(|| EngineError::Internal("lazy Rust module was already installed".into()))
    }
}

impl<State> Clone for ResolvedRustModule<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            module: Arc::clone(&self.module),
        }
    }
}

impl<State> fmt::Debug for ResolvedRustModule<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.module.lock() {
            Ok(module) if module.is_some() => "available",
            Ok(_) => "installed",
            Err(_) => "poisoned",
        };
        f.debug_struct("ResolvedRustModule")
            .field("state", &state)
            .finish()
    }
}

/// Fully resolved module identity and contents produced by an importer.
///
/// The ID is the canonical identity used for caching and symbol qualification;
/// the content is the source or AST that will be parsed, qualified, and loaded
/// into the module system.
#[derive(Clone, Debug)]
pub struct ResolvedModule<State: Clone + Send + Sync + 'static = ()> {
    pub id: ModuleId,
    pub content: ResolvedModuleContent<State>,
}

/// Deterministic compact key derived from a [`ModuleId`].
///
/// The engine uses module keys to build stable internal symbol prefixes without
/// embedding full paths, URLs, or virtual module names in every canonical symbol.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ModuleKey(u64);

impl ModuleKey {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Namespace category for a symbol exported from a module.
///
/// Rex keeps values, type constructors, and type classes distinct, so one local
/// name can have separate canonical bindings in different symbol namespaces.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum SymbolKind {
    /// A term-level value or function.
    Value,
    /// A type constructor or type alias namespace entry.
    Type,
    /// A type class namespace entry.
    Class,
}

/// Canonical runtime identity for an exported Rex symbol.
///
/// It records the defining module, namespace, original local export name, and
/// globally unique interned symbol used after imports are rewritten.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct CanonicalSymbol {
    pub module: ModuleKey,
    pub kind: SymbolKind,
    pub local: Symbol,
    pub symbol: Symbol,
}

impl CanonicalSymbol {
    pub fn new(module: ModuleKey, kind: SymbolKind, local: Symbol) -> Self {
        let symbol = Symbol::intern(&format!(
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

/// Export slots for a single public name in a module.
///
/// A Rex name can simultaneously refer to distinct namespaces, so each entry
/// may contain a value, type, and/or class canonical symbol.
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

/// Public export table for a module.
///
/// The table maps each user-facing export name to the canonical value, type,
/// and class symbols that should be introduced when another module imports it.
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

/// Host-provided module staged in memory before the module system imports it.
///
/// Virtual modules are used for injected modules and built-ins. They carry the
/// declarations that define the module surface used by import typechecking.
#[derive(Clone)]
pub struct VirtualModule {
    pub package: CompilationPackage,
}

pub fn module_key_for_module(id: &ModuleId) -> ModuleKey {
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

    hash_bytes(state, b"module:");
    for segment in id.segments() {
        hash_bytes(state, segment.as_bytes());
        hash_bytes(state, b".");
    }
}

pub fn prefix_for_module_key(key: ModuleKey) -> String {
    format!("@m{:016x}", key.as_u64())
}

pub fn prefix_for_module(id: &ModuleId) -> String {
    prefix_for_module_key(module_key_for_module(id))
}

pub fn qualify(prefix: &str, name: &Symbol) -> Symbol {
    Symbol::intern(&format!("{prefix}.{}", name.as_ref()))
}

pub fn virtual_export_name(module: &str, export: &str) -> String {
    match ModuleId::parse(module) {
        Ok(id) => {
            let key = module_key_for_module(&id);
            CanonicalSymbol::new(key, SymbolKind::Value, Symbol::intern(export))
                .symbol()
                .to_string()
        }
        Err(_) => format!("{module}.{export}"),
    }
}
