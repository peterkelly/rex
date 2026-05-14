use crate::env::Environment;
use rex_ast::Symbol;
use rex_typesystem::types::{Scheme, Type, TypedExpr};
use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    sync::Arc,
};

pub(crate) const RUNTIME_LINK_ABI_VERSION: u32 = 1;

/// Prepared Rex code plus the process-local environment needed to run it.
///
/// A compiled program is not a serialization artifact. It captures a typed
/// expression, an environment snapshot, and a runtime link contract that must
/// be validated against an evaluator before execution.
pub struct CompiledProgram {
    /// Name-level summary of external runtime bindings referenced by this program.
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

    /// Inferred result type of the prepared expression.
    pub fn result_type(&self) -> &Type {
        &self.expr.typ
    }

    /// Name-level summary of external native and class-method references.
    pub fn externs(&self) -> &CompiledExterns {
        &self.externs
    }

    /// Type-aware runtime link requirements for this program.
    pub fn link_contract(&self) -> &RuntimeLinkContract {
        &self.link_contract
    }

    /// Stable fingerprint for the type-aware runtime link contract.
    pub fn link_fingerprint(&self) -> u64 {
        self.link_contract.fingerprint()
    }

    /// Describe the storage boundary for this process-local API artifact.
    pub fn storage_boundary(&self) -> CompiledProgramBoundary {
        CompiledProgramBoundary {
            contains_prepared_expr: true,
            captures_process_local_env: true,
            serializable: false,
        }
    }
}

/// Name-level summary of external bindings referenced by compiled code.
///
/// This is useful for display and coarse reporting. Use
/// [`CompiledProgram::link_contract`] with [`RuntimeEnv`](crate::RuntimeEnv) for
/// type-aware validation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompiledExterns {
    /// Native function names referenced by the prepared expression.
    pub natives: Vec<Symbol>,
    /// Typeclass method names referenced by the prepared expression.
    pub class_methods: Vec<Symbol>,
}

impl CompiledExterns {
    /// Return true when no external bindings were referenced.
    pub fn is_empty(&self) -> bool {
        self.natives.is_empty() && self.class_methods.is_empty()
    }

    /// Fingerprint this name-level extern summary.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "natives".hash(&mut hasher);
        self.natives.hash(&mut hasher);
        "class_methods".hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }

    /// Compare this name-level summary with runtime capabilities.
    ///
    /// This reports missing names only. Type-aware compatibility is performed by
    /// [`RuntimeEnv::compatibility_with`](crate::RuntimeEnv::compatibility_with)
    /// against a full [`CompiledProgram`].
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

/// Type-aware runtime ABI and callable requirements for a prepared program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeLinkContract {
    /// Runtime link ABI version expected by the prepared program.
    pub abi_version: u32,
    /// Native function implementations required by name and concrete call type.
    pub natives: Vec<NativeRequirement>,
    /// Typeclass method implementations required by name and concrete call type.
    pub class_methods: Vec<ClassMethodRequirement>,
}

impl RuntimeLinkContract {
    /// Fingerprint this link contract.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.abi_version.hash(&mut hasher);
        self.natives.hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }
}

/// Storage-boundary metadata for a [`CompiledProgram`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledProgramBoundary {
    /// Whether this value contains a prepared typed expression.
    pub contains_prepared_expr: bool,
    /// Whether this value captures process-local runtime environment state.
    pub captures_process_local_env: bool,
    /// Whether this value is currently safe to serialize and reload elsewhere.
    pub serializable: bool,
}

/// Structured compatibility report for a compiled program and runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCompatibility {
    /// ABI version required by the compiled program.
    pub expected_abi_version: u32,
    /// ABI version exposed by the runtime.
    pub actual_abi_version: u32,
    /// Required native names not present in the runtime.
    pub missing_natives: Vec<Symbol>,
    /// Required native names present but not type-compatible.
    pub incompatible_natives: Vec<Symbol>,
    /// Required class-method names not present in the runtime.
    pub missing_class_methods: Vec<Symbol>,
    /// Required class-method names present but not type-compatible.
    pub incompatible_class_methods: Vec<Symbol>,
}

impl RuntimeCompatibility {
    /// Return true when ABI, native, and class-method requirements all match.
    pub fn is_compatible(&self) -> bool {
        self.expected_abi_version == self.actual_abi_version
            && self.missing_natives.is_empty()
            && self.incompatible_natives.is_empty()
            && self.missing_class_methods.is_empty()
            && self.incompatible_class_methods.is_empty()
    }
}

/// Concrete native function requirement captured by a prepared program.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct NativeRequirement {
    /// Native function name.
    pub name: Symbol,
    /// Concrete type expected at the call site.
    pub typ: Type,
}

/// Concrete class-method requirement captured by a prepared program.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClassMethodRequirement {
    /// Class method name.
    pub name: Symbol,
    /// Concrete type expected at the call site.
    pub typ: Type,
}

/// Runtime-side capabilities available to satisfy a link contract.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeCapabilities {
    /// Runtime link ABI version.
    pub abi_version: u32,
    /// Native function names available in the runtime.
    pub natives: Vec<Symbol>,
    /// Class-method names available in the runtime.
    pub class_methods: Vec<Symbol>,
    pub(crate) native_impls: BTreeMap<Symbol, Vec<NativeCapability>>,
    pub(crate) class_method_impls: BTreeMap<Symbol, ClassMethodCapability>,
}

impl RuntimeCapabilities {
    /// Fingerprint the name-level runtime capability summary.
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

/// Runtime-side metadata for one native implementation.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeCapability {
    /// Native function name.
    pub name: Symbol,
    /// Number of arguments admitted by this implementation.
    pub arity: usize,
    /// Polymorphic Rex type scheme implemented by this native.
    pub scheme: Scheme,
}

/// Runtime-side metadata for one class-method implementation.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassMethodCapability {
    /// Class method name.
    pub name: Symbol,
    /// Polymorphic Rex type scheme implemented by this method.
    pub scheme: Scheme,
}
