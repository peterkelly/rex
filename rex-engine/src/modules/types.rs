use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use rex_ast::expr::{Symbol, intern};

use crate::Value;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleId {
    Local { path: PathBuf, hash: String },
    Remote(String),
    Virtual(String),
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleId::Local { path, hash } => write!(f, "file:{}#{hash}", path.display()),
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

#[derive(Clone)]
pub struct ModuleExports {
    pub values: HashMap<Symbol, Symbol>,
    pub types: HashMap<Symbol, Symbol>,
    pub classes: HashMap<Symbol, Symbol>,
}

#[derive(Clone, Default)]
pub struct ReplState {
    pub(crate) alias_exports: HashMap<Symbol, ModuleExports>,
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
    pub init_value: Value,
}

pub(crate) fn prefix_for_module(id: &ModuleId) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.to_string().hash(&mut hasher);
    let h = hasher.finish();
    format!("@m{h:016x}")
}

pub(crate) fn qualify(prefix: &str, name: &Symbol) -> Symbol {
    intern(&format!("{prefix}.{}", name.as_ref()))
}

pub fn virtual_export_name(module: &str, export: &str) -> String {
    let id = ModuleId::Virtual(module.to_string());
    let prefix = prefix_for_module(&id);
    format!("{prefix}.{export}")
}
