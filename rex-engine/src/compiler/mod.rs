use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use rex_ast::{CompilationUnit, Expr, FnDecl, Symbol, Var};
use rex_typesystem::types::{Predicate, Type, TypeKind, TypedExpr, TypedExprKind};
use rex_typesystem::typesystem::TypeSystem;
use uuid::Uuid;

use crate::{
    CompileError, EngineError, Environment, Evaluator,
    builder::{engine::Engine, rewrite::rewrite_program_with_imports},
    compiler::{
        program::{CompiledExterns, CompiledProgram},
        type_check::{collect_pattern_bindings, type_check_engine},
    },
    manifest::{MainInputSpec, MainSignature},
    modules::{
        ImportChain, ImportRequest, Importer, ModuleExports, ModuleId, exports_from_program,
        parse_program_from_source, prefix_for_module, program_from_resolved,
    },
};

pub(crate) mod program;
pub(crate) mod type_check;

/// Options for compiling an already parsed Rex program.
#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    /// Path used to resolve relative imports in the program.
    pub importer_path: Option<PathBuf>,
    /// Source identity used to qualify top-level declarations.
    pub prefix_source: Option<ModuleId>,
}

impl CompileOptions {
    /// Use `path` as the anchor for resolving relative imports.
    pub fn with_importer_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.importer_path = Some(path.into());
        self
    }

    /// Use `source` to qualify top-level declarations.
    pub fn with_prefix_source(mut self, source: ModuleId) -> Self {
        self.prefix_source = Some(source);
        self
    }
}

/// Compile-time view of a prepared Rex engine.
///
/// A compiler owns the engine state needed for import rewriting, declaration
/// injection, and typechecking. Convert it into an [`Evaluator`] once all
/// programs you need from that preparation state have been compiled.
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

    /// Consume this compiler and build a single-shot evaluator.
    pub fn into_evaluator(self) -> Evaluator<State> {
        let runtime = self.engine.runtime_core();
        Evaluator::new(runtime)
    }

    /// Borrow the compiler's type system snapshot.
    pub fn type_system(&self) -> &TypeSystem {
        &self.engine.type_system
    }

    /// Typecheck an expression and package it as a prepared program.
    pub fn compile_expr(&mut self, expr: &Expr) -> Result<CompiledProgram, CompileError> {
        let typed = self.type_check(expr).map_err(CompileError::from)?;
        let signature = MainSignature::new(Vec::new(), typed.typ.clone());
        Ok(self.compile_typed_expr(typed, signature))
    }

    fn compile_typed_expr(
        &self,
        typed: TypedExpr,
        main_signature: MainSignature,
    ) -> CompiledProgram {
        let env = self.engine.env_snapshot();
        let externs = self.collect_externs(&typed, &env);
        CompiledProgram::new(externs, main_signature, env, typed)
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
            let rewritten = rewrite_program_with_imports(
                &mut self.engine,
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

    /// Rewrite imports for, typecheck, and prepare an already-parsed program.
    ///
    /// Programs are compiled using Rex's external `main` semantics: a program
    /// with `fn main ...` exposes that function's parameters as runtime inputs;
    /// otherwise a final expression is treated as an implicit zero-input main.
    pub async fn compile_program(
        &mut self,
        program: &CompilationUnit,
        options: CompileOptions,
    ) -> Result<CompiledProgram, CompileError> {
        let entry = main_entry_program(program)?;
        let importer = options.importer_path.map(|p| ModuleId::Local { path: p });
        let prefix = options
            .prefix_source
            .map(|id| prefix_for_module(&id))
            .unwrap_or_else(|| format!("@snippet{}", Uuid::new_v4()));
        let mut loaded: BTreeMap<ModuleId, ModuleExports> = BTreeMap::new();
        let mut loading: BTreeSet<ModuleId> = BTreeSet::new();
        let chain = self.engine.modules.import_chain();
        let rewritten = self
            .rewrite_and_inject_program(
                &entry.program,
                importer,
                &prefix,
                &chain,
                &mut loaded,
                &mut loading,
            )
            .await?;
        let body = rewritten
            .body
            .ok_or(EngineError::MissingBody { context: "program" })?;
        let typed = self.type_check(body.as_ref())?;
        let main_signature = main_signature_for_type(entry.param_names, &typed.typ)?;
        Ok(self.compile_typed_expr(typed, main_signature))
    }

    pub async fn infer_snippet(
        &mut self,
        source: &str,
        importer_path: Option<&Path>,
    ) -> Result<(Vec<Predicate>, Type), CompileError> {
        let program = parse_program_from_source(source, None).map_err(CompileError::from)?;
        let importer = importer_path.map(|p| ModuleId::Local {
            path: p.to_path_buf(),
        });
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
        self.engine
            .infer_type(body.as_ref())
            .map_err(CompileError::from)
    }

    pub async fn infer_module_with_importer(
        &mut self,
        request: ImportRequest,
        importer: Arc<dyn Importer>,
    ) -> Result<(Vec<Predicate>, Type), CompileError> {
        let chain = self.engine.modules.import_chain().with_importer(importer);
        let resolved = chain.import(request).await.map_err(CompileError::from)?;
        let mut loaded: BTreeMap<ModuleId, ModuleExports> = BTreeMap::new();
        let mut loading: BTreeSet<ModuleId> = BTreeSet::new();

        loading.insert(resolved.id.clone());

        let prefix = prefix_for_module(&resolved.id);
        let program = program_from_resolved(&resolved).map_err(CompileError::from)?;

        let rewritten = self
            .rewrite_and_inject_program(
                &program,
                Some(resolved.id.clone()),
                &prefix,
                &chain,
                &mut loaded,
                &mut loading,
            )
            .await
            .map_err(CompileError::from)?;
        let body = rewritten
            .body
            .unwrap_or_else(|| Arc::new(Expr::Tuple(Default::default(), Vec::new())));
        let result = self
            .engine
            .infer_type(body.as_ref())
            .map_err(CompileError::from)?;

        let exports = exports_from_program(&program, &prefix, &resolved.id);
        loaded.insert(resolved.id.clone(), exports);
        loading.remove(&resolved.id);

        Ok(result)
    }
}

struct MainEntryProgram {
    program: CompilationUnit,
    param_names: Vec<String>,
}

fn main_entry_program(program: &CompilationUnit) -> Result<MainEntryProgram, EngineError> {
    let main_decl = program.get_fn_decl("main");
    match (main_decl, program.body.as_ref()) {
        (Some(_), Some(_)) => Err(EngineError::MainWithFinalExpression),
        (Some(main_decl), None) => Ok(MainEntryProgram {
            program: CompilationUnit {
                decls: program.decls.clone(),
                body: Some(Arc::new(Expr::Var(Var::new("main")))),
            },
            param_names: main_param_names(main_decl)?,
        }),
        (None, Some(body)) => Ok(MainEntryProgram {
            program: CompilationUnit {
                decls: program.decls.clone(),
                body: Some(Arc::clone(body)),
            },
            param_names: Vec::new(),
        }),
        (None, None) => Err(EngineError::MissingMain),
    }
}

fn main_param_names(main_decl: &FnDecl) -> Result<Vec<String>, EngineError> {
    let mut seen = BTreeSet::new();
    let mut names = Vec::with_capacity(main_decl.params.len());
    for (var, _) in &main_decl.params {
        let name = var.name.to_string();
        if !seen.insert(name.clone()) {
            return Err(EngineError::DuplicateMainInput { name });
        }
        names.push(name);
    }
    Ok(names)
}

fn main_signature_for_type(
    param_names: Vec<String>,
    typ: &Type,
) -> Result<MainSignature, EngineError> {
    let declared = param_names.len();
    let mut cur = typ.clone();
    let mut inputs = Vec::with_capacity(declared);

    for name in param_names {
        let TypeKind::Fun(param, ret) = cur.as_ref() else {
            return Err(EngineError::MainArityMismatch {
                declared,
                inferred: inputs.len(),
            });
        };
        inputs.push(MainInputSpec {
            name,
            typ: param.clone(),
        });
        cur = ret.clone();
    }

    Ok(MainSignature::new(inputs, cur))
}
