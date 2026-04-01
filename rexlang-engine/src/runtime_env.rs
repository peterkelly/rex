use crate::engine::{
    CompiledProgram, Engine, RuntimeCapabilities, RuntimeCompatibility, RuntimeLinkContract,
    RuntimeSnapshot, class_method_capability_matches_requirement,
    native_capability_matches_requirement,
};
use crate::{EngineError, EvalError};

#[derive(Clone)]
pub struct RuntimeEnv<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) loader: Engine<State>,
    pub(crate) runtime: RuntimeSnapshot<State>,
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

impl<State> RuntimeEnv<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub fn new(engine: Engine<State>) -> Self {
        let capabilities = engine.runtime_capabilities_snapshot();
        let runtime = engine.runtime_snapshot();
        Self {
            loader: engine,
            runtime,
            capabilities,
        }
    }

    pub fn capabilities(&self) -> &RuntimeCapabilities {
        &self.capabilities
    }

    pub fn fingerprint(&self) -> u64 {
        self.capabilities.fingerprint()
    }

    pub fn compatibility_with(&self, program: &CompiledProgram) -> RuntimeCompatibility {
        runtime_compatibility(program.link_contract(), &self.capabilities)
    }

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

    pub(crate) fn sync_from_engine(&mut self, engine: &Engine<State>) {
        self.loader = engine.clone();
        self.runtime = engine.runtime_snapshot();
        self.capabilities = engine.runtime_capabilities_snapshot();
    }

    pub fn storage_boundary(&self) -> RuntimeEnvBoundary {
        RuntimeEnvBoundary {
            contains_runtime_snapshot: true,
            contains_loader_state: true,
            serializable: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeEnvBoundary {
    pub contains_runtime_snapshot: bool,
    pub contains_loader_state: bool,
    pub serializable: bool,
}
