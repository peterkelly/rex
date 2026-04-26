use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use rex_ast::expr::{Expr, Program, Symbol, sym};
use rex_typesystem::{
    error::TypeError,
    types::{Type, TypedExpr, Types},
    unification::{Subst, unify},
};
use rex_util::{GasMeter, sha256_hex};

use crate::engine::{
    CompiledProgram, NativeImpl, OverloadedFn, RuntimeSnapshot, check_runtime_cancelled,
    eval_typed_expr, impl_matches_type, is_function_type, type_head_is_var,
};
use crate::modules::{ModuleId, ReplState, ResolvedModule, ResolvedModuleContent};
use crate::{
    CompileError, Compiler, EngineError, Environment, EvalError, ExecutionError, Pointer,
    RuntimeEnv,
};

pub struct Evaluator<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) runtime: RuntimeEnv<State>,
    pub(crate) compiler: Option<Compiler<State>>,
}

#[derive(Clone)]
pub struct EvaluatorRef<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    runtime: RuntimeSnapshot<State>,
    #[allow(dead_code)]
    #[doc(hidden)]
    pub context: EvalContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct EvalContext {
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
    pub fn new(runtime: RuntimeEnv<State>) -> Self {
        Self {
            runtime,
            compiler: None,
        }
    }

    pub fn new_with_compiler(runtime: RuntimeEnv<State>, compiler: Compiler<State>) -> Self {
        Self {
            runtime,
            compiler: Some(compiler),
        }
    }

    fn sync_runtime_from_compiler(&mut self) {
        if let Some(compiler) = &self.compiler {
            self.runtime.sync_from_engine(&compiler.engine);
        }
    }

    pub async fn run(
        &mut self,
        program: &CompiledProgram,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EvalError> {
        check_runtime_cancelled(&self.runtime.runtime)?;
        self.runtime.validate_internal(program)?;
        eval_typed_expr(
            &self.runtime.runtime,
            &program.env,
            program.expr.as_ref(),
            gas,
        )
        .await
        .map_err(EvalError::from)
    }

    pub async fn eval(
        &mut self,
        expr: &Expr,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        self.prepare_and_run(gas, |compiler, _gas| compiler.compile_expr(expr))
            .await
    }

    async fn run_prepared(
        &mut self,
        program: CompiledProgram,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        self.sync_runtime_from_compiler();
        let typ = program.result_type().clone();
        let value = self.run(&program, gas).await?;
        Ok((value, typ))
    }

    async fn prepare_and_run<F>(
        &mut self,
        gas: &mut GasMeter,
        compile: F,
    ) -> Result<(Pointer, Type), ExecutionError>
    where
        F: FnOnce(&mut Compiler<State>, &mut GasMeter) -> Result<CompiledProgram, CompileError>,
    {
        let compiler = self.compiler.as_mut().ok_or_else(|| {
            CompileError::from(EngineError::Internal("evaluator has no compiler".into()))
        })?;
        let program = compile(compiler, gas)?;
        self.run_prepared(program, gas).await
    }

    pub async fn eval_module_file(
        &mut self,
        path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        let (id, bytes) = self
            .runtime
            .loader
            .read_local_module_bytes(path.as_ref())
            .map_err(CompileError::from)?;
        let source_fingerprint = sha256_hex(&bytes);
        if let Some(inst) = self
            .runtime
            .loader
            .modules
            .cached(&id)
            .map_err(EvalError::from)?
        {
            if inst.source_fingerprint.as_deref() == Some(source_fingerprint.as_str()) {
                return Ok((inst.init_value, inst.init_type));
            }
            self.runtime
                .loader
                .invalidate_module_caches(&id)
                .map_err(EvalError::from)?;
        }
        let source = self
            .runtime
            .loader
            .decode_local_module_source(&id, bytes)
            .map_err(CompileError::from)?;
        let inst = self
            .runtime
            .loader
            .load_module_from_resolved(
                ResolvedModule {
                    id,
                    content: ResolvedModuleContent::Source(source),
                },
                gas,
            )
            .await
            .map_err(CompileError::from)?;
        Ok((inst.init_value, inst.init_type))
    }

    pub async fn eval_module_source(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        let id = ModuleId::Virtual(format!("<inline:{:016x}>", hasher.finish()));
        if let Some(inst) = self
            .runtime
            .loader
            .modules
            .cached(&id)
            .map_err(EvalError::from)?
        {
            return Ok((inst.init_value, inst.init_type));
        }
        let inst = self
            .runtime
            .loader
            .load_module_from_resolved(
                ResolvedModule {
                    id,
                    content: ResolvedModuleContent::Source(source.to_string()),
                },
                gas,
            )
            .await
            .map_err(CompileError::from)?;
        Ok((inst.init_value, inst.init_type))
    }

    pub async fn eval_snippet(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        self.prepare_and_run(gas, |compiler, gas| compiler.compile_snippet(source, gas))
            .await
    }

    pub async fn eval_repl_program(
        &mut self,
        program: &Program,
        state: &mut ReplState,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        let compiler = self.compiler.as_mut().ok_or_else(|| {
            CompileError::from(EngineError::Internal("evaluator has no compiler".into()))
        })?;
        let compiled = compiler.compile_repl_program(program, state, gas).await?;
        self.run_prepared(compiled, gas).await
    }

    pub async fn eval_snippet_at(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        let path = importer_path.as_ref().to_path_buf();
        self.prepare_and_run(gas, |compiler, gas| {
            compiler.compile_snippet_at(source, &path, gas)
        })
        .await
    }
}

impl<State> EvaluatorRef<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new_with_context(runtime: &RuntimeSnapshot<State>, context: EvalContext) -> Self {
        Self {
            runtime: runtime.clone(),
            context,
        }
    }

    pub(crate) fn new_with_parent(runtime: &RuntimeSnapshot<State>, parent: Pointer) -> Self {
        Self::new_with_context(runtime, EvalContext::child(parent))
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
                        .alloc_overloaded(name, typ, applied, applied_types)?;
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
        let sym_name = sym(name);
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

    pub(crate) fn resolve_native(
        &self,
        name: &str,
        typ: &Type,
        _gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let sym_name = sym(name);
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
                let (native_id, name, arity, typ, gas_cost, applied, applied_types) =
                    imp.to_native_fn(typ.clone()).into_parts();
                self.runtime.heap.alloc_native(
                    native_id,
                    name,
                    arity,
                    typ,
                    gas_cost,
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
                        .alloc_overloaded(name, typ, applied, applied_types)
                } else {
                    Err(EngineError::AmbiguousOverload { name: sym_name })
                }
            }
        }
    }
}

impl<State> Deref for EvaluatorRef<State>
where
    State: Clone + Send + Sync + 'static,
{
    type Target = RuntimeSnapshot<State>;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}
