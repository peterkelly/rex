use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rexlang_ast::expr::{Expr, Program, Symbol};
use rexlang_typesystem::types::{TypedExpr, TypedExprKind};
use rexlang_util::GasMeter;
use uuid::Uuid;

use crate::engine::{
    ClassMethodRequirement, CompiledExterns, CompiledProgram, Engine, NativeRequirement,
    RUNTIME_LINK_ABI_VERSION, RuntimeLinkContract, collect_pattern_bindings, type_check_engine,
};
use crate::libraries::{
    LibraryExports, LibraryId, ReplState, ResolvedLibrary, decl_type_names, decl_value_names,
    exports_from_program, parse_program_from_source, prefix_for_library, rewrite_import_uses,
    validate_import_uses,
};
use crate::{CompileError, EngineError, Env};

#[derive(Clone)]
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
    pub fn new(engine: Engine<State>) -> Self {
        Self { engine }
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

    fn collect_externs(&self, expr: &TypedExpr, env: &Env) -> CompiledExterns {
        enum Frame<'b> {
            Expr(&'b TypedExpr),
            Push(Symbol),
            PushMany(Vec<Symbol>),
            Pop(usize),
        }

        let mut natives = BTreeSet::new();
        let mut class_methods = BTreeSet::new();
        let mut bound: Vec<Symbol> = Vec::new();
        let mut stack = vec![Frame::Expr(expr)];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Expr(expr) => match &expr.kind {
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
                            stack.push(Frame::Expr(elem));
                        }
                    }
                    TypedExprKind::Dict(kvs) => {
                        for v in kvs.values().rev() {
                            stack.push(Frame::Expr(v));
                        }
                    }
                    TypedExprKind::RecordUpdate { base, updates } => {
                        for v in updates.values().rev() {
                            stack.push(Frame::Expr(v));
                        }
                        stack.push(Frame::Expr(base));
                    }
                    TypedExprKind::App(f, x) => {
                        stack.push(Frame::Expr(x));
                        stack.push(Frame::Expr(f));
                    }
                    TypedExprKind::Project { expr, .. } => stack.push(Frame::Expr(expr)),
                    TypedExprKind::Lam { param, body } => {
                        stack.push(Frame::Pop(1));
                        stack.push(Frame::Expr(body));
                        stack.push(Frame::Push(param.clone()));
                    }
                    TypedExprKind::Let { name, def, body } => {
                        stack.push(Frame::Pop(1));
                        stack.push(Frame::Expr(body));
                        stack.push(Frame::Push(name.clone()));
                        stack.push(Frame::Expr(def));
                    }
                    TypedExprKind::LetRec { bindings, body } => {
                        if !bindings.is_empty() {
                            stack.push(Frame::Pop(bindings.len()));
                            stack.push(Frame::Expr(body));
                            for (_, def) in bindings.iter().rev() {
                                stack.push(Frame::Expr(def));
                            }
                            stack.push(Frame::PushMany(
                                bindings.iter().map(|(name, _)| name.clone()).collect(),
                            ));
                        } else {
                            stack.push(Frame::Expr(body));
                        }
                    }
                    TypedExprKind::Ite {
                        cond,
                        then_expr,
                        else_expr,
                    } => {
                        stack.push(Frame::Expr(else_expr));
                        stack.push(Frame::Expr(then_expr));
                        stack.push(Frame::Expr(cond));
                    }
                    TypedExprKind::Match { scrutinee, arms } => {
                        for (pat, arm_expr) in arms.iter().rev() {
                            let mut bindings = Vec::new();
                            collect_pattern_bindings(pat, &mut bindings);
                            let count = bindings.len();
                            if count != 0 {
                                stack.push(Frame::Pop(count));
                                stack.push(Frame::Expr(arm_expr));
                                stack.push(Frame::PushMany(bindings));
                            } else {
                                stack.push(Frame::Expr(arm_expr));
                            }
                        }
                        stack.push(Frame::Expr(scrutinee));
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
                Frame::Push(sym) => bound.push(sym),
                Frame::PushMany(syms) => bound.extend(syms),
                Frame::Pop(count) => bound.truncate(bound.len().saturating_sub(count)),
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

    fn link_contract(&self, expr: &TypedExpr, env: &Env) -> RuntimeLinkContract {
        enum Frame<'b> {
            Expr(&'b TypedExpr),
            Push(Symbol),
            PushMany(Vec<Symbol>),
            Pop(usize),
        }

        let mut native_requirements = BTreeSet::new();
        let mut class_method_requirements = BTreeSet::new();
        let mut bound: Vec<Symbol> = Vec::new();
        let mut stack = vec![Frame::Expr(expr)];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Expr(expr) => match &expr.kind {
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
                            stack.push(Frame::Expr(elem));
                        }
                    }
                    TypedExprKind::Dict(kvs) => {
                        for v in kvs.values().rev() {
                            stack.push(Frame::Expr(v));
                        }
                    }
                    TypedExprKind::RecordUpdate { base, updates } => {
                        for v in updates.values().rev() {
                            stack.push(Frame::Expr(v));
                        }
                        stack.push(Frame::Expr(base));
                    }
                    TypedExprKind::App(f, x) => {
                        stack.push(Frame::Expr(x));
                        stack.push(Frame::Expr(f));
                    }
                    TypedExprKind::Project { expr, .. } => stack.push(Frame::Expr(expr)),
                    TypedExprKind::Lam { param, body } => {
                        stack.push(Frame::Pop(1));
                        stack.push(Frame::Expr(body));
                        stack.push(Frame::Push(param.clone()));
                    }
                    TypedExprKind::Let { name, def, body } => {
                        stack.push(Frame::Pop(1));
                        stack.push(Frame::Expr(body));
                        stack.push(Frame::Push(name.clone()));
                        stack.push(Frame::Expr(def));
                    }
                    TypedExprKind::LetRec { bindings, body } => {
                        if !bindings.is_empty() {
                            stack.push(Frame::Pop(bindings.len()));
                            stack.push(Frame::Expr(body));
                            for (_, def) in bindings.iter().rev() {
                                stack.push(Frame::Expr(def));
                            }
                            stack.push(Frame::PushMany(
                                bindings.iter().map(|(name, _)| name.clone()).collect(),
                            ));
                        } else {
                            stack.push(Frame::Expr(body));
                        }
                    }
                    TypedExprKind::Ite {
                        cond,
                        then_expr,
                        else_expr,
                    } => {
                        stack.push(Frame::Expr(else_expr));
                        stack.push(Frame::Expr(then_expr));
                        stack.push(Frame::Expr(cond));
                    }
                    TypedExprKind::Match { scrutinee, arms } => {
                        for (pat, arm_expr) in arms.iter().rev() {
                            let mut bindings = Vec::new();
                            collect_pattern_bindings(pat, &mut bindings);
                            let count = bindings.len();
                            if count != 0 {
                                stack.push(Frame::Pop(count));
                                stack.push(Frame::Expr(arm_expr));
                                stack.push(Frame::PushMany(bindings));
                            } else {
                                stack.push(Frame::Expr(arm_expr));
                            }
                        }
                        stack.push(Frame::Expr(scrutinee));
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
                Frame::Push(sym) => bound.push(sym),
                Frame::PushMany(syms) => bound.extend(syms),
                Frame::Pop(count) => bound.truncate(bound.len().saturating_sub(count)),
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

    fn rewrite_and_inject_program(
        &mut self,
        program: &Program,
        importer: Option<LibraryId>,
        prefix: &str,
        gas: &mut GasMeter,
        loaded: &mut BTreeMap<LibraryId, LibraryExports>,
        loading: &mut BTreeSet<LibraryId>,
    ) -> Result<Program, EngineError> {
        let rewritten = self
            .engine
            .rewrite_program_with_imports(program, importer, prefix, gas, loaded, loading)?;
        self.engine.inject_decls(&rewritten.decls)?;
        Ok(rewritten)
    }

    pub fn compile_snippet(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<CompiledProgram, CompileError> {
        self.compile_snippet_with_gas_and_importer(source, gas, None)
            .map_err(CompileError::from)
    }

    pub fn compile_snippet_at(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<CompiledProgram, CompileError> {
        let path = importer_path.as_ref().to_path_buf();
        self.compile_snippet_with_gas_and_importer(source, gas, Some(path))
            .map_err(CompileError::from)
    }

    pub fn compile_library_file(
        &mut self,
        path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<CompiledProgram, CompileError> {
        let (id, bytes) = self
            .engine
            .read_local_library_bytes(path.as_ref())
            .map_err(CompileError::from)?;
        let source = self
            .engine
            .decode_local_library_source(&id, bytes)
            .map_err(CompileError::from)?;
        self.compile_library_source(
            ResolvedLibrary {
                id,
                content: crate::libraries::ResolvedLibraryContent::Source(source),
            },
            gas,
        )
        .map_err(CompileError::from)
    }

    pub async fn compile_repl_program(
        &mut self,
        program: &Program,
        state: &mut ReplState,
        gas: &mut GasMeter,
    ) -> Result<CompiledProgram, CompileError> {
        self.compile_repl_program_internal(program, state, gas)
            .await
            .map_err(CompileError::from)
    }

    async fn compile_repl_program_internal(
        &mut self,
        program: &Program,
        state: &mut ReplState,
        gas: &mut GasMeter,
    ) -> Result<CompiledProgram, EngineError> {
        let importer = state
            .importer_path
            .as_ref()
            .map(|p| LibraryId::Local { path: p.clone() });

        let mut local_values = state.defined_values.clone();
        local_values.extend(decl_value_names(&program.decls));
        let local_types = decl_type_names(&program.decls);
        let existing_imported: BTreeSet<Symbol> = state.imported_values.keys().cloned().collect();
        let existing_imported_types: BTreeSet<Symbol> =
            state.imported_types.keys().cloned().collect();
        let existing_imported_classes: BTreeSet<Symbol> =
            state.imported_classes.keys().cloned().collect();
        let import_policy = crate::libraries::ImportBindingPolicy {
            forbidden_values: &local_values,
            forbidden_types: &local_types,
            existing_imported_values: Some(&existing_imported),
            existing_imported_types: Some(&existing_imported_types),
            existing_imported_classes: Some(&existing_imported_classes),
        };
        let import_bindings = self
            .engine
            .import_bindings_for_decls(&program.decls, importer.clone(), &import_policy, gas)
            .await?;
        state.alias_exports.extend(import_bindings.alias_exports);
        state
            .imported_values
            .extend(import_bindings.imported_values);
        state.imported_types.extend(import_bindings.imported_types);
        state
            .imported_classes
            .extend(import_bindings.imported_classes);

        let mut shadowed_values = state.defined_values.clone();
        shadowed_values.extend(decl_value_names(&program.decls));
        let shadowed_types = decl_type_names(&program.decls);

        validate_import_uses(program, &state.alias_exports, Some(&shadowed_values))?;
        let rewritten = rewrite_import_uses(
            program,
            &state.alias_exports,
            &state.imported_values,
            &state.imported_types,
            &state.imported_classes,
            Some(&shadowed_types),
            Some(&shadowed_values),
        );

        self.engine.inject_decls(&rewritten.decls)?;
        state
            .defined_values
            .extend(decl_value_names(&program.decls));
        self.compile_expr_internal(rewritten.expr.as_ref())
    }

    fn compile_library_source(
        &mut self,
        resolved: ResolvedLibrary,
        gas: &mut GasMeter,
    ) -> Result<CompiledProgram, EngineError> {
        let mut loaded: BTreeMap<LibraryId, LibraryExports> = BTreeMap::new();
        let mut loading: BTreeSet<LibraryId> = BTreeSet::new();

        loading.insert(resolved.id.clone());

        let prefix = prefix_for_library(&resolved.id);
        let program = crate::libraries::program_from_resolved(&resolved, &mut *gas)?;
        let rewritten = self.rewrite_and_inject_program(
            &program,
            Some(resolved.id.clone()),
            &prefix,
            gas,
            &mut loaded,
            &mut loading,
        )?;

        let exports = exports_from_program(&program, &prefix, &resolved.id);
        loaded.insert(resolved.id.clone(), exports);
        loading.remove(&resolved.id);

        self.compile_expr_internal(rewritten.expr.as_ref())
    }

    fn compile_snippet_with_gas_and_importer(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
        importer_path: Option<PathBuf>,
    ) -> Result<CompiledProgram, EngineError> {
        let program = parse_program_from_source(source, None, Some(&mut *gas))?;

        let importer = importer_path.map(|p| LibraryId::Local { path: p });
        let prefix = format!("@snippet{}", Uuid::new_v4());
        let mut loaded: BTreeMap<LibraryId, LibraryExports> = BTreeMap::new();
        let mut loading: BTreeSet<LibraryId> = BTreeSet::new();
        let rewritten = self.rewrite_and_inject_program(
            &program,
            importer,
            &prefix,
            gas,
            &mut loaded,
            &mut loading,
        )?;
        self.compile_expr_internal(rewritten.expr.as_ref())
    }
}
