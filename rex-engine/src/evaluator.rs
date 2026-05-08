use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use rex_ast::{Expr, Symbol};
use rex_typesystem::{
    error::TypeError,
    types::{Type, TypedExpr, Types},
    typesystem::TypeSystem,
    unification::{Subst, unify},
};
use rex_util::sha256_hex;

use crate::engine::{
    CompiledProgram, NativeImpl, OverloadedFn, RuntimeCapabilities, RuntimeCompatibility,
    RuntimeCore, eval_typed_expr, impl_matches_type, is_function_type, type_head_is_var,
};
use crate::modules::{ModuleId, ResolvedModule, ResolvedModuleContent};
use crate::value::{Handle, Heap, Pointer};
use crate::{
    CompileError, Compiler, EngineError, Environment, EvalError, ExecutionError, RuntimeEnv,
};

pub struct Evaluator<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) runtime: RuntimeCore<State>,
    pub(crate) runtime_env: RuntimeEnv,
    pub(crate) compiler: Compiler<State>,
}

#[derive(Clone)]
pub struct EvaluatorRef<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    runtime: RuntimeCore<State>,
    #[allow(dead_code)]
    #[doc(hidden)]
    pub(crate) context: EvalContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub(crate) struct EvalContext {
    pub parent: Option<Pointer>,
}

impl EvalContext {
    pub(crate) fn child(parent: Pointer) -> Self {
        Self {
            parent: Some(parent),
        }
    }
}

impl<State> Evaluator<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(
        runtime: RuntimeCore<State>,
        runtime_env: RuntimeEnv,
        compiler: Compiler<State>,
    ) -> Self {
        Self {
            runtime,
            runtime_env,
            compiler,
        }
    }

    pub fn runtime_env(&self) -> &RuntimeEnv {
        &self.runtime_env
    }

    pub fn type_system(&self) -> Arc<TypeSystem> {
        Arc::clone(&self.runtime.type_system)
    }

    pub fn heap(&self) -> &Heap {
        &self.runtime.heap
    }

    pub fn capabilities(&self) -> &RuntimeCapabilities {
        self.runtime_env.capabilities()
    }

    pub fn compatibility_with(&self, program: &CompiledProgram) -> RuntimeCompatibility {
        self.runtime_env.compatibility_with(program)
    }

    pub fn validate(&self, program: &CompiledProgram) -> Result<(), EvalError> {
        self.runtime_env.validate(program)
    }

    pub async fn run(self, program: CompiledProgram) -> Result<Handle, EvalError> {
        self.runtime_env.validate_internal(&program)?;
        let runtime = self.runtime;
        let heap = runtime.heap.clone();
        let pointer = eval_typed_expr(runtime, program.env, program.expr)
            .await
            .map_err(EvalError::from)?;
        heap.handle(pointer).map_err(EvalError::from)
    }

    pub async fn eval(self, expr: &Expr) -> Result<(Handle, Type), ExecutionError> {
        self.prepare_and_run(|compiler| compiler.compile_expr(expr))
            .await
    }

    async fn run_prepared(
        mut self,
        program: CompiledProgram,
    ) -> Result<(Handle, Type), ExecutionError> {
        self.runtime_env = RuntimeEnv::from_engine(&self.compiler.engine);
        self.runtime = self.compiler.engine.runtime_core();
        let typ = program.result_type().clone();
        let value = self.run(program).await?;
        Ok((value, typ))
    }

    async fn prepare_and_run<F>(mut self, compile: F) -> Result<(Handle, Type), ExecutionError>
    where
        F: FnOnce(&mut Compiler<State>) -> Result<CompiledProgram, CompileError>,
    {
        let program = compile(&mut self.compiler)?;
        self.run_prepared(program).await
    }

    pub async fn eval_module_file(
        mut self,
        path: impl AsRef<Path>,
    ) -> Result<(Handle, Type), ExecutionError> {
        let result: Result<(Handle, Type), ExecutionError> = {
            let engine = &mut self.compiler.engine;
            let (id, bytes) = engine
                .read_local_module_bytes(path.as_ref())
                .map_err(CompileError::from)?;
            let source_fingerprint = sha256_hex(&bytes);
            if let Some(inst) = engine.modules.cached(&id).map_err(EvalError::from)? {
                if inst.source_fingerprint.as_deref() == Some(source_fingerprint.as_str()) {
                    Ok((inst.init_value, inst.init_type))
                } else {
                    engine
                        .invalidate_module_caches(&id)
                        .map_err(EvalError::from)?;
                    let source = engine
                        .decode_local_module_source(&id, bytes)
                        .map_err(CompileError::from)?;
                    let inst = engine
                        .load_module_from_resolved(ResolvedModule {
                            id,
                            content: ResolvedModuleContent::Source(source),
                        })
                        .await
                        .map_err(CompileError::from)?;
                    Ok((inst.init_value, inst.init_type))
                }
            } else {
                let source = engine
                    .decode_local_module_source(&id, bytes)
                    .map_err(CompileError::from)?;
                let inst = engine
                    .load_module_from_resolved(ResolvedModule {
                        id,
                        content: ResolvedModuleContent::Source(source),
                    })
                    .await
                    .map_err(CompileError::from)?;
                Ok((inst.init_value, inst.init_type))
            }
        };
        result
    }

    pub async fn eval_module_source(
        mut self,
        source: &str,
    ) -> Result<(Handle, Type), ExecutionError> {
        let result: Result<(Handle, Type), ExecutionError> = {
            let engine = &mut self.compiler.engine;
            let mut hasher = DefaultHasher::new();
            source.hash(&mut hasher);
            let id = ModuleId::Virtual(format!("<inline:{:016x}>", hasher.finish()));
            if let Some(inst) = engine.modules.cached(&id).map_err(EvalError::from)? {
                Ok((inst.init_value, inst.init_type))
            } else {
                let inst = engine
                    .load_module_from_resolved(ResolvedModule {
                        id,
                        content: ResolvedModuleContent::Source(source.to_string()),
                    })
                    .await
                    .map_err(CompileError::from)?;
                Ok((inst.init_value, inst.init_type))
            }
        };
        result
    }

    pub async fn eval_snippet(self, source: &str) -> Result<(Handle, Type), ExecutionError> {
        self.prepare_and_run(|compiler| compiler.compile_snippet(source))
            .await
    }

    pub async fn eval_snippet_at(
        self,
        source: &str,
        importer_path: impl AsRef<Path>,
    ) -> Result<(Handle, Type), ExecutionError> {
        let path = importer_path.as_ref().to_path_buf();
        self.prepare_and_run(|compiler| compiler.compile_snippet_at(source, &path))
            .await
    }
}

impl<State> EvaluatorRef<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new_with_context(runtime: &RuntimeCore<State>, context: EvalContext) -> Self {
        Self {
            runtime: runtime.clone(),
            context,
        }
    }

    pub(crate) fn new_with_parent(runtime: &RuntimeCore<State>, parent: Pointer) -> Self {
        Self::new_with_context(runtime, EvalContext::child(parent))
    }

    pub fn state(&self) -> &State {
        self.runtime.state.as_ref()
    }

    pub fn heap(&self) -> &Heap {
        &self.runtime.heap
    }

    pub fn type_system(&self) -> &TypeSystem {
        self.runtime.type_system.as_ref()
    }

    pub(crate) fn handles_from_pointers(
        &self,
        pointers: &[Pointer],
    ) -> Result<Vec<Handle>, EngineError> {
        pointers
            .iter()
            .map(|pointer| self.runtime.heap.handle(*pointer))
            .collect()
    }

    fn resolve_typeclass_method_impl(
        &self,
        name: &Symbol,
        call_type: &Type,
    ) -> Result<(Environment, Arc<TypedExpr>, Subst), EngineError> {
        let info = self
            .runtime
            .type_system
            .class_methods
            .get(name)
            .ok_or_else(|| EngineError::UnknownVar(name.clone()))?;

        let s_method = unify(&info.scheme.typ, call_type).map_err(EngineError::Type)?;
        let class_pred = info
            .scheme
            .preds
            .iter()
            .find(|p| p.class == info.class)
            .ok_or(EngineError::Type(TypeError::UnsupportedExpr(
                "method scheme missing class predicate",
            )))?;
        let param_type = class_pred.typ.apply(&s_method);
        if type_head_is_var(&param_type) {
            return Err(EngineError::AmbiguousOverload { name: name.clone() });
        }

        self.runtime
            .typeclasses
            .resolve(&info.class, name, &param_type)
    }

    pub(crate) fn cached_class_method(&self, name: &Symbol, typ: &Type) -> Option<Pointer> {
        if !typ.ftv().is_empty() {
            return None;
        }
        let cache = self.runtime.typeclass_cache.lock().ok()?;
        cache.get(&(name.clone(), typ.clone())).cloned()
    }

    pub(crate) fn resolve_class_method_plan(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<Result<(Environment, TypedExpr), Pointer>, EngineError> {
        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                let (name, typ, applied, applied_types) =
                    OverloadedFn::new(name.clone(), typ.clone()).into_parts();
                let pointer =
                    self.runtime
                        .heap
                        .alloc_ptr_overloaded(name, typ, applied, applied_types)?;
                return Ok(Err(pointer));
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        Ok(Ok((def_env, specialized)))
    }

    pub(crate) fn resolve_native_impl(
        &self,
        name: &str,
        typ: &Type,
    ) -> Result<NativeImpl<State>, EngineError> {
        let sym_name = Symbol::intern(name);
        let impls = self
            .runtime
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => Ok(matches[0].clone()),
            _ => Err(EngineError::AmbiguousImpl {
                name: sym_name,
                typ: typ.to_string(),
            }),
        }
    }

    pub(crate) fn resolve_native(&self, name: &str, typ: &Type) -> Result<Pointer, EngineError> {
        let sym_name = Symbol::intern(name);
        let impls = self
            .runtime
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => {
                let imp = matches[0].clone();
                let (native_id, name, arity, typ, applied, applied_types) =
                    imp.to_native_fn(typ.clone()).into_parts();
                self.runtime.heap.alloc_ptr_native(
                    native_id,
                    name,
                    arity,
                    typ,
                    applied,
                    applied_types,
                )
            }
            _ => {
                if typ.ftv().is_empty() {
                    Err(EngineError::AmbiguousImpl {
                        name: sym_name.clone(),
                        typ: typ.to_string(),
                    })
                } else if is_function_type(typ) {
                    let (name, typ, applied, applied_types) =
                        OverloadedFn::new(sym_name.clone(), typ.clone()).into_parts();
                    self.runtime
                        .heap
                        .alloc_ptr_overloaded(name, typ, applied, applied_types)
                } else {
                    Err(EngineError::AmbiguousOverload { name: sym_name })
                }
            }
        }
    }
}
