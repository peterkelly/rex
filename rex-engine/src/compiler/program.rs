use crate::env::Environment;
use rex_ast::Symbol;
use rex_typesystem::types::{Scheme, Type, TypedExpr};
use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    sync::Arc,
};

pub(crate) const RUNTIME_LINK_ABI_VERSION: u32 = 1;

pub struct CompiledProgram {
    pub externs: CompiledExterns,
    link_contract: RuntimeLinkContract,
    pub(crate) env: Environment,
    pub(crate) expr: Arc<TypedExpr>,
}

impl CompiledProgram {
    pub(crate) fn new(
        externs: CompiledExterns,
        link_contract: RuntimeLinkContract,
        env: Environment,
        expr: TypedExpr,
    ) -> Self {
        Self {
            externs,
            link_contract,
            env,
            expr: Arc::new(expr),
        }
    }

    pub fn result_type(&self) -> &Type {
        &self.expr.typ
    }

    pub fn externs(&self) -> &CompiledExterns {
        &self.externs
    }

    pub fn link_contract(&self) -> &RuntimeLinkContract {
        &self.link_contract
    }

    pub fn link_fingerprint(&self) -> u64 {
        self.link_contract.fingerprint()
    }

    pub fn storage_boundary(&self) -> CompiledProgramBoundary {
        CompiledProgramBoundary {
            contains_prepared_expr: true,
            captures_process_local_env: true,
            serializable: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompiledExterns {
    pub natives: Vec<Symbol>,
    pub class_methods: Vec<Symbol>,
}

impl CompiledExterns {
    pub fn is_empty(&self) -> bool {
        self.natives.is_empty() && self.class_methods.is_empty()
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "natives".hash(&mut hasher);
        self.natives.hash(&mut hasher);
        "class_methods".hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }

    pub fn compatibility_with(&self, capabilities: &RuntimeCapabilities) -> RuntimeCompatibility {
        let natives = capabilities
            .natives
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let class_methods = capabilities
            .class_methods
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        RuntimeCompatibility {
            expected_abi_version: capabilities.abi_version,
            actual_abi_version: capabilities.abi_version,
            missing_natives: self
                .natives
                .iter()
                .filter(|name| !natives.contains(*name))
                .cloned()
                .collect(),
            incompatible_natives: Vec::new(),
            missing_class_methods: self
                .class_methods
                .iter()
                .filter(|name| !class_methods.contains(*name))
                .cloned()
                .collect(),
            incompatible_class_methods: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeLinkContract {
    pub abi_version: u32,
    pub natives: Vec<NativeRequirement>,
    pub class_methods: Vec<ClassMethodRequirement>,
}

impl RuntimeLinkContract {
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.abi_version.hash(&mut hasher);
        self.natives.hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledProgramBoundary {
    pub contains_prepared_expr: bool,
    pub captures_process_local_env: bool,
    pub serializable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCompatibility {
    pub expected_abi_version: u32,
    pub actual_abi_version: u32,
    pub missing_natives: Vec<Symbol>,
    pub incompatible_natives: Vec<Symbol>,
    pub missing_class_methods: Vec<Symbol>,
    pub incompatible_class_methods: Vec<Symbol>,
}

impl RuntimeCompatibility {
    pub fn is_compatible(&self) -> bool {
        self.expected_abi_version == self.actual_abi_version
            && self.missing_natives.is_empty()
            && self.incompatible_natives.is_empty()
            && self.missing_class_methods.is_empty()
            && self.incompatible_class_methods.is_empty()
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct NativeRequirement {
    pub name: Symbol,
    pub typ: Type,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClassMethodRequirement {
    pub name: Symbol,
    pub typ: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeCapabilities {
    pub abi_version: u32,
    pub natives: Vec<Symbol>,
    pub class_methods: Vec<Symbol>,
    pub(crate) native_impls: BTreeMap<Symbol, Vec<NativeCapability>>,
    pub(crate) class_method_impls: BTreeMap<Symbol, ClassMethodCapability>,
}

impl RuntimeCapabilities {
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.abi_version.hash(&mut hasher);
        "natives".hash(&mut hasher);
        self.natives.hash(&mut hasher);
        "class_methods".hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeCapability {
    pub name: Symbol,
    pub arity: usize,
    pub scheme: Scheme,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassMethodCapability {
    pub name: Symbol,
    pub scheme: Scheme,
}
