use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use rex_ast::{CompilationUnit, Decl, Symbol};
use rex_typesystem::types::Type;
use rex_util::sha256_hex;

use crate::Handle;

/// Stable identity for a Rex module as seen by the runtime module system.
///
/// Module IDs are used as cache keys, cycle-detection keys, and the source of
/// deterministic prefixes for canonical internal symbols.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum ModuleId {
    /// A module loaded from a local filesystem path.
    Local { path: PathBuf },
    /// A module loaded from a remote locator such as a URL.
    Remote(String),
    /// A host-provided or built-in module that has no source file identity.
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

/// Request passed from the module system to importers when Rex code imports a module.
///
/// It carries the requested module name plus the optional identity of the
/// importing module so importers can resolve relative names or enforce policy.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ImportRequest {
    pub module_name: String,
    pub importer: Option<ModuleId>,
}

impl ImportRequest {
    pub fn new(module_name: impl Into<String>) -> Self {
        Self {
            module_name: module_name.into(),
            importer: None,
        }
    }

    pub fn with_importer(module_name: impl Into<String>, importer: ModuleId) -> Self {
        Self {
            module_name: module_name.into(),
            importer: Some(importer),
        }
    }
}

/// Imported module payload returned by an [`Importer`](super::Importer).
///
/// Importers may return raw Rex source for the engine to parse or a prebuilt
/// compilation unit when the caller has already parsed or synthesized the AST.
#[derive(Clone, Debug)]
pub enum ResolvedModuleContent {
    /// Raw Rex source text.
    Source(String),
    /// Parsed or synthesized module declarations.
    CompilationUnit(CompilationUnit),
}

/// Fully resolved module identity and contents produced by an importer.
///
/// The ID is the canonical identity used for caching and symbol qualification;
/// the content is the source or AST that will be compiled and evaluated.
#[derive(Clone, Debug)]
pub struct ResolvedModule {
    pub id: ModuleId,
    pub content: ResolvedModuleContent,
}

impl ResolvedModule {
    fn source_fingerprint(source: &str) -> String {
        sha256_hex(source.as_bytes())
    }

    pub(crate) fn content_fingerprint(&self) -> Option<String> {
        match &self.content {
            ResolvedModuleContent::Source(source) => Some(Self::source_fingerprint(source)),
            ResolvedModuleContent::CompilationUnit(_) => None,
        }
    }
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
/// public export table, the declarations that define those exports, and optional
/// source text for diagnostics or documentation rendering.
#[derive(Clone)]
pub struct VirtualModule {
    pub exports: ModuleExports,
    pub decls: Vec<Decl>,
    pub source: Option<String>,
}

/// Loaded module cached by the module system after compilation and evaluation.
///
/// A module instance stores its canonical identity, export table, runtime
/// initialization handle, inferred initialization type, and optional source
/// fingerprint so later imports can reuse the completed module safely.
#[derive(Clone)]
pub struct ModuleInstance {
    pub id: ModuleId,
    pub exports: ModuleExports,
    pub init_value: Handle,
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
    Symbol::intern(&format!("{prefix}.{}", name.as_ref()))
}

pub fn virtual_export_name(module: &str, export: &str) -> String {
    let id = ModuleId::Virtual(module.to_string());
    let key = module_key_for_module(&id);
    CanonicalSymbol::new(key, SymbolKind::Value, Symbol::intern(export))
        .symbol()
        .to_string()
}
