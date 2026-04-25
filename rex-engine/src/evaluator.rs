use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use async_recursion::async_recursion;
use rex_ast::expr::{Expr, Program, Symbol, sym};
use rex_typesystem::{
    error::TypeError,
    types::{Type, TypedExpr, Types},
    unification::{Subst, unify},
};
use rex_util::{GasMeter, sha256_hex};

use crate::engine::{
    CompiledProgram, NativeImpl, OverloadedFn, RuntimeSnapshot, apply_with_context,
    check_runtime_cancelled, eval_typed_expr, eval_typed_expr_loop, eval_typed_expr_loop_child,
    impl_matches_type, is_function_type, type_head_is_var,
};
use crate::modules::{ModuleId, ReplState, ResolvedModule, ResolvedModuleContent};
use crate::value::Value;
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

#[derive(Clone, Copy)]
pub struct EvaluatorRef<'a, State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    runtime: &'a RuntimeSnapshot<State>,
    context: EvalContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvalBackend {
    Recursive,
    Loop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EvalContext {
    backend: EvalBackend,
    loop_parent: Option<Pointer>,
}

impl EvalContext {
    pub(crate) fn recursive() -> Self {
        Self {
            backend: EvalBackend::Recursive,
            loop_parent: None,
        }
    }

    pub(crate) fn loop_root() -> Self {
        Self {
            backend: EvalBackend::Loop,
            loop_parent: None,
        }
    }

    pub(crate) fn loop_child(parent: Pointer) -> Self {
        Self {
            backend: EvalBackend::Loop,
            loop_parent: Some(parent),
        }
    }

    pub(crate) fn from_backend(backend: EvalBackend) -> Self {
        match backend {
            EvalBackend::Recursive => Self::recursive(),
            EvalBackend::Loop => Self::loop_root(),
        }
    }

    pub(crate) fn backend(self) -> EvalBackend {
        self.backend
    }

    pub(crate) fn loop_parent(self) -> Option<Pointer> {
        self.loop_parent
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

    pub(crate) fn sync_runtime_from_compiler(&mut self) {
        if let Some(compiler) = &self.compiler {
            self.runtime.sync_from_engine(&compiler.engine);
        }
    }

    pub async fn run(
        &mut self,
        program: &CompiledProgram,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EvalError> {
        self.run_internal(program, gas)
            .await
            .map_err(EvalError::from)
    }

    pub(crate) async fn run_internal(
        &mut self,
        program: &CompiledProgram,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        check_runtime_cancelled(&self.runtime.runtime)?;
        self.runtime.validate_internal(program)?;
        eval_typed_expr_loop(
            &self.runtime.runtime,
            &program.env,
            program.expr.as_ref(),
            gas,
        )
        .await
    }

    pub async fn eval(
        &mut self,
        expr: &Expr,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        self.prepare_and_run(gas, |compiler, _gas| compiler.compile_expr(expr))
            .await
    }

    pub async fn run_prepared(
        &mut self,
        program: CompiledProgram,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), ExecutionError> {
        self.sync_runtime_from_compiler();
        let typ = program.result_type().clone();
        let value = self.run(&program, gas).await?;
        Ok((value, typ))
    }

    pub(crate) async fn prepare_and_run<F>(
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

impl<'a, State> EvaluatorRef<'a, State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(runtime: &'a RuntimeSnapshot<State>) -> Self {
        Self {
            runtime,
            context: EvalContext::recursive(),
        }
    }

    pub(crate) fn new_with_context(
        runtime: &'a RuntimeSnapshot<State>,
        context: EvalContext,
    ) -> Self {
        Self { runtime, context }
    }

    pub(crate) fn new_with_loop_parent(
        runtime: &'a RuntimeSnapshot<State>,
        parent: Pointer,
    ) -> Self {
        Self::new_with_context(runtime, EvalContext::loop_child(parent))
    }

    #[cfg(test)]
    pub(crate) fn backend(&self) -> EvalBackend {
        self.context.backend()
    }

    #[cfg(test)]
    pub(crate) fn loop_parent(&self) -> Option<Pointer> {
        self.context.loop_parent()
    }

    #[async_recursion]
    async fn eval_typed_expr(
        &self,
        env: &Environment,
        expr: &TypedExpr,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        match self.context.backend() {
            EvalBackend::Recursive => eval_typed_expr(self.runtime, env, expr, gas).await,
            EvalBackend::Loop => match self.context.loop_parent() {
                Some(parent) => {
                    eval_typed_expr_loop_child(self.runtime, parent, env, expr, gas).await
                }
                None => eval_typed_expr_loop(self.runtime, env, expr, gas).await,
            },
        }
    }

    pub(crate) async fn apply_pointer(
        &self,
        func: Pointer,
        arg: Pointer,
        func_type: Option<&Type>,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        apply_with_context(
            self.runtime,
            func,
            arg,
            func_type,
            arg_type,
            gas,
            self.context,
        )
        .await
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

    fn cached_class_method(&self, name: &Symbol, typ: &Type) -> Option<Pointer> {
        if !typ.ftv().is_empty() {
            return None;
        }
        let cache = self.runtime.typeclass_cache.lock().ok()?;
        cache.get(&(name.clone(), typ.clone())).cloned()
    }

    fn insert_cached_class_method(&self, name: &Symbol, typ: &Type, pointer: &Pointer) {
        if typ.ftv().is_empty()
            && let Ok(mut cache) = self.runtime.typeclass_cache.lock()
        {
            cache.insert((name.clone(), typ.clone()), *pointer);
        }
    }

    fn resolve_class_method_plan(
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

    #[async_recursion]
    pub(crate) async fn resolve_class_method(
        &self,
        name: &Symbol,
        typ: &Type,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        if let Some(pointer) = self.cached_class_method(name, typ) {
            return Ok(pointer);
        }

        let pointer = match self.resolve_class_method_plan(name, typ)? {
            Ok((def_env, specialized)) => self.eval_typed_expr(&def_env, &specialized, gas).await?,
            Err(pointer) => pointer,
        };

        if typ.ftv().is_empty() {
            self.insert_cached_class_method(name, typ, &pointer);
        }
        Ok(pointer)
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

    pub(crate) async fn resolve_global(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<Pointer, EngineError> {
        if let Some(ptr) = self.runtime.env.get(name) {
            let value = self.runtime.heap.get(&ptr)?;
            match value.as_ref() {
                Value::Native(native) if native.is_zero_unapplied() => {
                    let mut gas = GasMeter::default();
                    native
                        .call_zero_with_context(self.runtime, &mut gas, self.context)
                        .await
                }
                _ => Ok(ptr),
            }
        } else if self.runtime.type_system.class_methods.contains_key(name) {
            let mut gas = GasMeter::default();
            self.resolve_class_method(name, typ, &mut gas).await
        } else {
            let mut gas = GasMeter::default();
            let pointer = self.resolve_native(name.as_ref(), typ, &mut gas)?;
            let value = self.runtime.heap.get(&pointer)?;
            match value.as_ref() {
                Value::Native(native) if native.is_zero_unapplied() => {
                    let mut gas = GasMeter::default();
                    native
                        .call_zero_with_context(self.runtime, &mut gas, self.context)
                        .await
                }
                _ => Ok(pointer),
            }
        }
    }

    pub(crate) async fn call_native_impl(
        &self,
        name: &str,
        typ: &Type,
        args: &[Pointer],
    ) -> Result<Pointer, EngineError> {
        let imp = self.resolve_native_impl(name, typ)?;
        imp.func
            .call_with_context(self.runtime, typ.clone(), args, self.context)
            .await
    }
}

impl<'a, State> Deref for EvaluatorRef<'a, State>
where
    State: Clone + Send + Sync + 'static,
{
    type Target = RuntimeSnapshot<State>;

    fn deref(&self) -> &Self::Target {
        self.runtime
    }
}
