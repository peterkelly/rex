use crate::{
    EngineError, EvalError,
    builder::engine::Engine,
    compiler::program::{
        ClassMethodCapability, ClassMethodRequirement, CompiledProgram, NativeCapability,
        NativeRequirement, RuntimeCapabilities, RuntimeCompatibility, RuntimeLinkContract,
    },
    util::type_arity,
};
use rex_typesystem::{
    typesystem::{TypeVarSupply, instantiate},
    unification::unify,
};

/// Lightweight runtime link-capability snapshot for preflight validation.
///
/// `RuntimeEnv` deliberately does not contain evaluator runtime state or module
/// loader state. It is a process-local API artifact used to compare a
/// [`CompiledProgram`] against the runtime capabilities that would execute it.
pub struct RuntimeEnv {
    capabilities: RuntimeCapabilities,
}

fn runtime_compatibility(
    contract: &RuntimeLinkContract,
    capabilities: &RuntimeCapabilities,
) -> RuntimeCompatibility {
    let mut missing_natives = Vec::new();
    let mut incompatible_natives = Vec::new();
    for requirement in &contract.natives {
        match capabilities.native_impls.get(&requirement.name) {
            None => missing_natives.push(requirement.name.clone()),
            Some(impls) => {
                if !impls.iter().any(|capability| {
                    native_capability_matches_requirement(capability, requirement)
                }) {
                    incompatible_natives.push(requirement.name.clone());
                }
            }
        }
    }

    let mut missing_class_methods = Vec::new();
    let mut incompatible_class_methods = Vec::new();
    for requirement in &contract.class_methods {
        match capabilities.class_method_impls.get(&requirement.name) {
            None => missing_class_methods.push(requirement.name.clone()),
            Some(capability) => {
                if !class_method_capability_matches_requirement(capability, requirement) {
                    incompatible_class_methods.push(requirement.name.clone());
                }
            }
        }
    }

    RuntimeCompatibility {
        expected_abi_version: contract.abi_version,
        actual_abi_version: capabilities.abi_version,
        missing_natives,
        incompatible_natives,
        missing_class_methods,
        incompatible_class_methods,
    }
}

impl RuntimeEnv {
    pub(crate) fn from_engine<State>(engine: &Engine<State>) -> Self
    where
        State: Clone + Send + Sync + 'static,
    {
        let capabilities = engine.runtime_capabilities_snapshot();
        Self { capabilities }
    }

    /// Runtime capabilities captured by this preflight snapshot.
    pub fn capabilities(&self) -> &RuntimeCapabilities {
        &self.capabilities
    }

    /// Fingerprint the name-level capability summary.
    pub fn fingerprint(&self) -> u64 {
        self.capabilities.fingerprint()
    }

    /// Compare this runtime snapshot with a prepared program's link contract.
    pub fn compatibility_with(&self, program: &CompiledProgram) -> RuntimeCompatibility {
        runtime_compatibility(program.link_contract(), &self.capabilities)
    }

    /// Validate that this runtime snapshot can satisfy a prepared program.
    pub fn validate(&self, program: &CompiledProgram) -> Result<(), EvalError> {
        self.validate_internal(program).map_err(EvalError::from)
    }

    pub(crate) fn validate_internal(&self, program: &CompiledProgram) -> Result<(), EngineError> {
        let compatibility = self.compatibility_with(program);
        if compatibility.is_compatible() {
            Ok(())
        } else {
            Err(EngineError::Link {
                expected_abi_version: compatibility.expected_abi_version,
                actual_abi_version: compatibility.actual_abi_version,
                missing_natives: compatibility.missing_natives,
                incompatible_natives: compatibility.incompatible_natives,
                missing_class_methods: compatibility.missing_class_methods,
                incompatible_class_methods: compatibility.incompatible_class_methods,
            })
        }
    }

    /// Describe the storage boundary for this process-local API artifact.
    pub fn storage_boundary(&self) -> RuntimeEnvBoundary {
        RuntimeEnvBoundary {
            contains_runtime_core: false,
            contains_loader_state: false,
            serializable: false,
        }
    }
}

/// Storage-boundary metadata for a [`RuntimeEnv`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeEnvBoundary {
    /// Whether this value contains evaluator runtime state.
    pub contains_runtime_core: bool,
    /// Whether this value contains module loader/cache state.
    pub contains_loader_state: bool,
    /// Whether this value is currently safe to serialize and reload elsewhere.
    pub serializable: bool,
}

fn native_capability_matches_requirement(
    capability: &NativeCapability,
    requirement: &NativeRequirement,
) -> bool {
    let mut supply = TypeVarSupply::new();
    let (_preds, scheme_ty) = instantiate(&capability.scheme, &mut supply);
    capability.name == requirement.name
        && capability.arity == type_arity(&requirement.typ)
        && unify(&scheme_ty, &requirement.typ).is_ok()
}

fn class_method_capability_matches_requirement(
    capability: &ClassMethodCapability,
    requirement: &ClassMethodRequirement,
) -> bool {
    let mut supply = TypeVarSupply::new();
    let (_preds, scheme_ty) = instantiate(&capability.scheme, &mut supply);
    capability.name == requirement.name && unify(&scheme_ty, &requirement.typ).is_ok()
}
