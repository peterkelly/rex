use std::collections::BTreeSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use rex_ast::{CompilationUnit, Expr, FnDecl, Var};
use rex_typesystem::types::{Predicate, Type, TypeKind, TypedExpr};
use rex_typesystem::typesystem::TypeSystem;

use crate::{
    EngineError, Evaluator,
    builder::{
        core::{Builder, ModuleLoaderState, RuntimePolicy, RuntimeRegistry},
        rewrite::load_module_types_from_resolved,
        rewrite::{ModuleLoadState, rewrite_program_with_imports},
    },
    compiler::{program::CompiledProgram, type_check::type_check_engine},
    env::RootedEnvironment,
    evaluator::runtime_core::RuntimeCore,
    manifest::{MainInputSpec, MainSignature},
    modules::{
        ImportChain, ImportRequest, Importer, ModuleId, ResolvedModuleContent,
        exports_from_program, parse_program_from_source, prefix_for_module, program_from_resolved,
    },
    value::Heap,
};

pub(crate) mod program;
pub(crate) mod type_check;

/// Options for compiling an already parsed Rex program.
#[derive(Clone, Debug)]
pub struct CompileOptions {
    /// Abstract module identity assigned to this program for import resolution
    /// and canonical symbol qualification.
    pub module_id: ModuleId,
}

impl CompileOptions {
    pub fn new(module_id: ModuleId) -> Self {
        Self { module_id }
    }

    pub fn for_module(module: impl AsRef<str>) -> Result<Self, EngineError> {
        Ok(Self::new(ModuleId::parse(module)?))
    }
}

/// Compile-time view of a prepared Rex engine.
///
/// A compiler owns the engine state needed for import rewriting, declaration
/// injection, and typechecking. It is single-use: compiling a program consumes
/// the compiler and returns the evaluator built from that preparation state.
pub struct Compiler<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) state: Arc<State>,
    pub(crate) env: RootedEnvironment,
    pub(crate) type_system: TypeSystem,
    pub(crate) runtime: RuntimeRegistry<State>,
    pub(crate) module_loader: ModuleLoaderState<State>,
    pub(crate) policy: RuntimePolicy,
    pub(crate) heap: Heap,
}

impl<State> Compiler<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn from_builder(builder: Builder<State>) -> Self {
        Self {
            state: builder.state,
            env: builder.env,
            type_system: builder.type_system,
            runtime: builder.runtime,
            module_loader: builder.module_loader,
            policy: builder.policy,
            heap: builder.heap,
        }
    }

    fn into_evaluator(self) -> Evaluator<State> {
        Evaluator::new(RuntimeCore {
            state: Arc::clone(&self.state),
            natives: Arc::new(self.runtime.natives.clone()),
            typeclasses: Arc::new(self.runtime.typeclasses.clone()),
            type_system: Arc::new(self.type_system.clone()),
            typeclass_cache: Arc::clone(&self.runtime.typeclass_cache),
            async_call_policy: self.policy.async_call_policy.clone(),
            parallelism_controller: Arc::clone(&self.policy.parallelism_controller),
            heap: self.heap.clone(),
        })
    }

    /// Borrow the compiler's type system snapshot.
    pub fn type_system(&self) -> &TypeSystem {
        &self.type_system
    }

    fn compile_typed_expr(
        &self,
        typed: TypedExpr,
        main_signature: MainSignature,
    ) -> CompiledProgram {
        let env = self.env.clone();
        CompiledProgram::new(main_signature, env, typed)
    }

    pub(crate) fn type_check(&mut self, expr: &Expr) -> Result<TypedExpr, EngineError> {
        type_check_engine(
            &mut self.type_system,
            &self.env,
            &self.runtime.natives,
            expr,
        )
    }

    fn rewrite_and_inject_program<'a>(
        &'a mut self,
        compilation_unit: &'a CompilationUnit,
        importer: Option<ModuleId>,
        prefix: &'a str,
        chain: &'a ImportChain<State>,
        load_state: &'a mut ModuleLoadState<State>,
    ) -> BoxFuture<'a, Result<CompilationUnit, EngineError>> {
        Box::pin(async move {
            let rewritten = rewrite_program_with_imports(
                self,
                compilation_unit,
                importer,
                prefix,
                chain,
                load_state,
            )
            .await?;
            self.inject_decls(&rewritten.decls)?;
            Ok(rewritten)
        })
    }

    /// Rewrite imports for, typecheck, and prepare an already-parsed program.
    ///
    /// Programs are compiled using Rex's external `main` semantics: a program
    /// with `fn main ...` exposes that function's parameters as runtime inputs;
    /// otherwise a final expression is treated as an implicit zero-input main.
    pub async fn compile_program(
        mut self,
        program: &CompilationUnit,
        options: CompileOptions,
    ) -> Result<(CompiledProgram, Evaluator<State>), EngineError> {
        let entry = main_entry_program(program)?;
        let module_id = options.module_id;
        let prefix = prefix_for_module(&module_id);
        let mut load_state = ModuleLoadState::default();
        let chain = self.module_loader.system.import_chain();
        let rewritten = self
            .rewrite_and_inject_program(
                &entry.program,
                Some(module_id),
                &prefix,
                &chain,
                &mut load_state,
            )
            .await?;
        let body = rewritten
            .body
            .ok_or(EngineError::MissingBody { context: "program" })?;
        let typed = self.type_check(body.as_ref())?;
        let main_signature = main_signature_for_type(entry.param_names, &typed.typ)?;
        let compiled = self.compile_typed_expr(typed, main_signature);
        let evaluator = self.into_evaluator();
        Ok((compiled, evaluator))
    }

    pub async fn infer_snippet(
        mut self,
        source: &str,
        module_id: ModuleId,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let program = parse_program_from_source(source, None)?;
        let prefix = prefix_for_module(&module_id);
        let mut load_state = ModuleLoadState::default();
        let chain = self.module_loader.system.import_chain();
        let rewritten = self
            .rewrite_and_inject_program(&program, Some(module_id), &prefix, &chain, &mut load_state)
            .await?;
        let body = rewritten
            .body
            .ok_or(EngineError::MissingBody { context: "snippet" })?;
        self.infer_type(body.as_ref())
    }

    pub async fn infer_module_with_importer(
        mut self,
        request: ImportRequest,
        importer: Arc<dyn Importer<State>>,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let chain = self
            .module_loader
            .system
            .import_chain()
            .with_importer(importer);
        let mut load_state = ModuleLoadState::default();
        let resolved = load_state.import(&chain, request).await?;

        if matches!(&resolved.content, ResolvedModuleContent::Module(_)) {
            load_module_types_from_resolved(&mut self, resolved, &chain, &mut load_state).await?;
            return Ok((Vec::new(), Type::tuple(Vec::<Type>::new())));
        }

        load_state.loading_mut().insert(resolved.id.clone());

        let prefix = prefix_for_module(&resolved.id);
        let program = program_from_resolved(&resolved)?;

        let rewritten = self
            .rewrite_and_inject_program(
                &program,
                Some(resolved.id.clone()),
                &prefix,
                &chain,
                &mut load_state,
            )
            .await?;
        let body = rewritten
            .body
            .unwrap_or_else(|| Arc::new(Expr::Tuple(Default::default(), Vec::new())));
        let result = self.infer_type(body.as_ref())?;

        let exports = exports_from_program(&program, &prefix, &resolved.id);
        load_state.loaded_mut().insert(resolved.id.clone(), exports);
        load_state.loading_mut().remove(&resolved.id);

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
