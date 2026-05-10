use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use rex_ast::{CompilationUnit, Expr, Symbol};
use rex_typesystem::types::{TypedExpr, TypedExprKind};
use uuid::Uuid;

use crate::engine::{
    ClassMethodRequirement, CompiledExterns, CompiledProgram, Engine, NativeRequirement,
    RUNTIME_LINK_ABI_VERSION, RuntimeLinkContract, collect_pattern_bindings, type_check_engine,
};
use crate::modules::{
    ImportChain, ImportRequest, Importer, ModuleExports, ModuleId, ResolvedModule,
    exports_from_program, parse_program_from_source, prefix_for_module,
};
use crate::{CompileError, EngineError, Environment, Evaluator, RuntimeEnv};

fn unit_expr() -> Arc<Expr> {
    Arc::new(Expr::Tuple(Default::default(), Vec::new()))
}

pub struct Compiler<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) engine: Engine<State>,
}

impl<State> Compiler<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(engine: Engine<State>) -> Self {
        Self { engine }
    }

    pub fn into_evaluator(self) -> Evaluator<State> {
        let runtime_env = RuntimeEnv::from_engine(&self.engine);
        let runtime = self.engine.runtime_core();
        Evaluator::new(runtime, runtime_env, self)
    }

    pub fn runtime_env(&self) -> RuntimeEnv {
        RuntimeEnv::from_engine(&self.engine)
    }

    pub fn compile_expr(&mut self, expr: &Expr) -> Result<CompiledProgram, CompileError> {
        self.compile_expr_internal(expr).map_err(CompileError::from)
    }

    pub(crate) fn compile_expr_internal(
        &mut self,
        expr: &Expr,
    ) -> Result<CompiledProgram, EngineError> {
        let typed = self.type_check(expr)?;
        let env = self.engine.env_snapshot();
        let externs = self.collect_externs(&typed, &env);
        let link_contract = self.link_contract(&typed, &env);
        Ok(CompiledProgram::new(externs, link_contract, env, typed))
    }

    pub(crate) fn type_check(&mut self, expr: &Expr) -> Result<TypedExpr, EngineError> {
        type_check_engine(&mut self.engine, expr)
    }

    fn collect_externs(&self, expr: &TypedExpr, env: &Environment) -> CompiledExterns {
        enum ScopeWalkStep<'b> {
            Expr(&'b TypedExpr),
            Push(Symbol),
            PushMany(Vec<Symbol>),
            Pop(usize),
        }

        let mut natives = BTreeSet::new();
        let mut class_methods = BTreeSet::new();
        let mut bound: Vec<Symbol> = Vec::new();
        let mut stack = vec![ScopeWalkStep::Expr(expr)];
        while let Some(frame) = stack.pop() {
            match frame {
                ScopeWalkStep::Expr(expr) => match expr.kind.as_ref() {
                    TypedExprKind::Var { name, .. } => {
                        if bound.iter().any(|sym| sym == name) || env.get(name).is_some() {
                            continue;
                        }
                        if self.engine.type_system.class_methods.contains_key(name) {
                            class_methods.insert(name.clone());
                        } else if self.engine.has_native_name(name) {
                            natives.insert(name.clone());
                        }
                    }
                    TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
                        for elem in elems.iter().rev() {
                            stack.push(ScopeWalkStep::Expr(elem));
                        }
                    }
                    TypedExprKind::Dict(kvs) => {
                        for v in kvs.values().rev() {
                            stack.push(ScopeWalkStep::Expr(v));
                        }
                    }
                    TypedExprKind::RecordUpdate { base, updates } => {
                        for v in updates.values().rev() {
                            stack.push(ScopeWalkStep::Expr(v));
                        }
                        stack.push(ScopeWalkStep::Expr(base));
                    }
                    TypedExprKind::App(f, x) => {
                        stack.push(ScopeWalkStep::Expr(x));
                        stack.push(ScopeWalkStep::Expr(f));
                    }
                    TypedExprKind::Project { expr, .. } => stack.push(ScopeWalkStep::Expr(expr)),
                    TypedExprKind::Lam { param, body } => {
                        stack.push(ScopeWalkStep::Pop(1));
                        stack.push(ScopeWalkStep::Expr(body));
                        stack.push(ScopeWalkStep::Push(param.clone()));
                    }
                    TypedExprKind::Let { name, def, body } => {
                        stack.push(ScopeWalkStep::Pop(1));
                        stack.push(ScopeWalkStep::Expr(body));
                        stack.push(ScopeWalkStep::Push(name.clone()));
                        stack.push(ScopeWalkStep::Expr(def));
                    }
                    TypedExprKind::LetRec { bindings, body } => {
                        if !bindings.is_empty() {
                            stack.push(ScopeWalkStep::Pop(bindings.len()));
                            stack.push(ScopeWalkStep::Expr(body));
                            for (_, def) in bindings.iter().rev() {
                                stack.push(ScopeWalkStep::Expr(def));
                            }
                            stack.push(ScopeWalkStep::PushMany(
                                bindings.iter().map(|(name, _)| name.clone()).collect(),
                            ));
                        } else {
                            stack.push(ScopeWalkStep::Expr(body));
                        }
                    }
                    TypedExprKind::Ite {
                        cond,
                        then_expr,
                        else_expr,
                    } => {
                        stack.push(ScopeWalkStep::Expr(else_expr));
                        stack.push(ScopeWalkStep::Expr(then_expr));
                        stack.push(ScopeWalkStep::Expr(cond));
                    }
                    TypedExprKind::Match { scrutinee, arms } => {
                        for (pat, arm_expr) in arms.iter().rev() {
                            let mut bindings = Vec::new();
                            collect_pattern_bindings(pat, &mut bindings);
                            let count = bindings.len();
                            if count != 0 {
                                stack.push(ScopeWalkStep::Pop(count));
                                stack.push(ScopeWalkStep::Expr(arm_expr));
                                stack.push(ScopeWalkStep::PushMany(bindings));
                            } else {
                                stack.push(ScopeWalkStep::Expr(arm_expr));
                            }
                        }
                        stack.push(ScopeWalkStep::Expr(scrutinee));
                    }
                    TypedExprKind::Bool(..)
                    | TypedExprKind::Uint(..)
                    | TypedExprKind::Int(..)
                    | TypedExprKind::Float(..)
                    | TypedExprKind::String(..)
                    | TypedExprKind::Uuid(..)
                    | TypedExprKind::DateTime(..)
                    | TypedExprKind::Hole => {}
                },
                ScopeWalkStep::Push(sym) => bound.push(sym),
                ScopeWalkStep::PushMany(syms) => bound.extend(syms),
                ScopeWalkStep::Pop(count) => bound.truncate(bound.len().saturating_sub(count)),
            }
        }

        let mut natives = natives.into_iter().collect::<Vec<_>>();
        let mut class_methods = class_methods.into_iter().collect::<Vec<_>>();
        natives.sort();
        class_methods.sort();
        CompiledExterns {
            natives,
            class_methods,
        }
    }

    fn link_contract(&self, expr: &TypedExpr, env: &Environment) -> RuntimeLinkContract {
        enum ScopeWalkStep<'b> {
            Expr(&'b TypedExpr),
            Push(Symbol),
            PushMany(Vec<Symbol>),
            Pop(usize),
        }

        let mut native_requirements = BTreeSet::new();
        let mut class_method_requirements = BTreeSet::new();
        let mut bound: Vec<Symbol> = Vec::new();
        let mut stack = vec![ScopeWalkStep::Expr(expr)];
        while let Some(frame) = stack.pop() {
            match frame {
                ScopeWalkStep::Expr(expr) => match expr.kind.as_ref() {
                    TypedExprKind::Var { name, .. } => {
                        if bound.iter().any(|sym| sym == name) || env.get(name).is_some() {
                            continue;
                        }
                        if self.engine.type_system.class_methods.contains_key(name) {
                            class_method_requirements.insert(ClassMethodRequirement {
                                name: name.clone(),
                                typ: expr.typ.clone(),
                            });
                        } else if self.engine.has_native_name(name) {
                            native_requirements.insert(NativeRequirement {
                                name: name.clone(),
                                typ: expr.typ.clone(),
                            });
                        }
                    }
                    TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
                        for elem in elems.iter().rev() {
                            stack.push(ScopeWalkStep::Expr(elem));
                        }
                    }
                    TypedExprKind::Dict(kvs) => {
                        for v in kvs.values().rev() {
                            stack.push(ScopeWalkStep::Expr(v));
                        }
                    }
                    TypedExprKind::RecordUpdate { base, updates } => {
                        for v in updates.values().rev() {
                            stack.push(ScopeWalkStep::Expr(v));
                        }
                        stack.push(ScopeWalkStep::Expr(base));
                    }
                    TypedExprKind::App(f, x) => {
                        stack.push(ScopeWalkStep::Expr(x));
                        stack.push(ScopeWalkStep::Expr(f));
                    }
                    TypedExprKind::Project { expr, .. } => stack.push(ScopeWalkStep::Expr(expr)),
                    TypedExprKind::Lam { param, body } => {
                        stack.push(ScopeWalkStep::Pop(1));
                        stack.push(ScopeWalkStep::Expr(body));
                        stack.push(ScopeWalkStep::Push(param.clone()));
                    }
                    TypedExprKind::Let { name, def, body } => {
                        stack.push(ScopeWalkStep::Pop(1));
                        stack.push(ScopeWalkStep::Expr(body));
                        stack.push(ScopeWalkStep::Push(name.clone()));
                        stack.push(ScopeWalkStep::Expr(def));
                    }
                    TypedExprKind::LetRec { bindings, body } => {
                        if !bindings.is_empty() {
                            stack.push(ScopeWalkStep::Pop(bindings.len()));
                            stack.push(ScopeWalkStep::Expr(body));
                            for (_, def) in bindings.iter().rev() {
                                stack.push(ScopeWalkStep::Expr(def));
                            }
                            stack.push(ScopeWalkStep::PushMany(
                                bindings.iter().map(|(name, _)| name.clone()).collect(),
                            ));
                        } else {
                            stack.push(ScopeWalkStep::Expr(body));
                        }
                    }
                    TypedExprKind::Ite {
                        cond,
                        then_expr,
                        else_expr,
                    } => {
                        stack.push(ScopeWalkStep::Expr(else_expr));
                        stack.push(ScopeWalkStep::Expr(then_expr));
                        stack.push(ScopeWalkStep::Expr(cond));
                    }
                    TypedExprKind::Match { scrutinee, arms } => {
                        for (pat, arm_expr) in arms.iter().rev() {
                            let mut bindings = Vec::new();
                            collect_pattern_bindings(pat, &mut bindings);
                            let count = bindings.len();
                            if count != 0 {
                                stack.push(ScopeWalkStep::Pop(count));
                                stack.push(ScopeWalkStep::Expr(arm_expr));
                                stack.push(ScopeWalkStep::PushMany(bindings));
                            } else {
                                stack.push(ScopeWalkStep::Expr(arm_expr));
                            }
                        }
                        stack.push(ScopeWalkStep::Expr(scrutinee));
                    }
                    TypedExprKind::Bool(..)
                    | TypedExprKind::Uint(..)
                    | TypedExprKind::Int(..)
                    | TypedExprKind::Float(..)
                    | TypedExprKind::String(..)
                    | TypedExprKind::Uuid(..)
                    | TypedExprKind::DateTime(..)
                    | TypedExprKind::Hole => {}
                },
                ScopeWalkStep::Push(sym) => bound.push(sym),
                ScopeWalkStep::PushMany(syms) => bound.extend(syms),
                ScopeWalkStep::Pop(count) => bound.truncate(bound.len().saturating_sub(count)),
            }
        }

        let mut natives = native_requirements.into_iter().collect::<Vec<_>>();
        let mut class_methods = class_method_requirements.into_iter().collect::<Vec<_>>();
        natives.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.typ.to_string().cmp(&b.typ.to_string()))
        });
        class_methods.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.typ.to_string().cmp(&b.typ.to_string()))
        });
        RuntimeLinkContract {
            abi_version: RUNTIME_LINK_ABI_VERSION,
            natives,
            class_methods,
        }
    }

    fn rewrite_and_inject_program<'a>(
        &'a mut self,
        compilation_unit: &'a CompilationUnit,
        importer: Option<ModuleId>,
        prefix: &'a str,
        chain: &'a ImportChain,
        loaded: &'a mut BTreeMap<ModuleId, ModuleExports>,
        loading: &'a mut BTreeSet<ModuleId>,
    ) -> BoxFuture<'a, Result<CompilationUnit, EngineError>> {
        Box::pin(async move {
            let rewritten = self
                .engine
                .rewrite_program_with_imports(
                    compilation_unit,
                    importer,
                    prefix,
                    chain,
                    loaded,
                    loading,
                )
                .await?;
            self.engine.inject_decls(&rewritten.decls)?;
            Ok(rewritten)
        })
    }

    pub async fn compile_snippet(&mut self, source: &str) -> Result<CompiledProgram, CompileError> {
        self.compile_snippet_with_importer(source, None)
            .await
            .map_err(CompileError::from)
    }

    pub async fn compile_snippet_at(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
    ) -> Result<CompiledProgram, CompileError> {
        let path = importer_path.as_ref().to_path_buf();
        self.compile_snippet_with_importer(source, Some(path))
            .await
            .map_err(CompileError::from)
    }

    pub async fn compile_module_with_importer(
        &mut self,
        request: ImportRequest,
        importer: Arc<dyn Importer>,
    ) -> Result<CompiledProgram, CompileError> {
        let chain = self.engine.modules.import_chain().with_importer(importer);
        let resolved = chain.import(request).await.map_err(CompileError::from)?;
        self.compile_module_source(resolved, &chain)
            .await
            .map_err(CompileError::from)
    }

    pub(crate) fn compile_module_source<'a>(
        &'a mut self,
        resolved: ResolvedModule,
        chain: &'a ImportChain,
    ) -> BoxFuture<'a, Result<CompiledProgram, EngineError>> {
        Box::pin(async move {
            let mut loaded: BTreeMap<ModuleId, ModuleExports> = BTreeMap::new();
            let mut loading: BTreeSet<ModuleId> = BTreeSet::new();

            loading.insert(resolved.id.clone());

            let prefix = prefix_for_module(&resolved.id);
            let program = crate::modules::program_from_resolved(&resolved)?;
            let rewritten = self
                .rewrite_and_inject_program(
                    &program,
                    Some(resolved.id.clone()),
                    &prefix,
                    chain,
                    &mut loaded,
                    &mut loading,
                )
                .await?;

            let exports = exports_from_program(&program, &prefix, &resolved.id);
            loaded.insert(resolved.id.clone(), exports);
            loading.remove(&resolved.id);

            let body = rewritten.body.unwrap_or_else(unit_expr);
            self.compile_expr_internal(body.as_ref())
        })
    }

    fn compile_snippet_with_importer<'a>(
        &'a mut self,
        source: &'a str,
        importer_path: Option<PathBuf>,
    ) -> BoxFuture<'a, Result<CompiledProgram, EngineError>> {
        Box::pin(async move {
            let program = parse_program_from_source(source, None)?;

            let importer = importer_path.map(|p| ModuleId::Local { path: p });
            let prefix = format!("@snippet{}", Uuid::new_v4());
            let mut loaded: BTreeMap<ModuleId, ModuleExports> = BTreeMap::new();
            let mut loading: BTreeSet<ModuleId> = BTreeSet::new();
            let chain = self.engine.modules.import_chain();
            let rewritten = self
                .rewrite_and_inject_program(
                    &program,
                    importer,
                    &prefix,
                    &chain,
                    &mut loaded,
                    &mut loading,
                )
                .await?;
            let body = rewritten
                .body
                .ok_or(EngineError::MissingBody { context: "snippet" })?;
            self.compile_expr_internal(body.as_ref())
        })
    }
}
