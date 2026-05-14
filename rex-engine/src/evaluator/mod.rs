use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use rex_ast::{Expr, Symbol};
use rex_typesystem::{
    types::{BuiltinTypeId, Type, Types},
    typesystem::TypeSystem,
    unification::unify,
};

use crate::{
    CompileError, Compiler, EvalError, ExecutionError, RuntimeEnv,
    compiler::program::{CompiledProgram, RuntimeCapabilities, RuntimeCompatibility},
    error::EngineError,
    evaluator::{eval::eval_typed_expr, runtime_core::RuntimeCore},
    modules::{ImportRequest, Importer, ModuleId, ResolvedModule, ResolvedModuleContent},
    util::split_fun,
    value::{Cell, Handle, Heap, HeapAccess, Pointer},
};

pub(crate) mod context;
pub(crate) mod eval;
pub(crate) mod native_callable;
pub(crate) mod native_functions;
pub(crate) mod runtime_core;
pub(crate) mod scheduler;

/// Single-shot runtime for validating and running prepared Rex code.
///
/// `run` consumes both the evaluator and the [`CompiledProgram`]. Convenience
/// helpers such as [`Evaluator::eval_snippet`] compile and run in one step, but
/// still consume the evaluator.
pub struct Evaluator<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) runtime: RuntimeCore<State>,
    pub(crate) runtime_env: RuntimeEnv,
    pub(crate) compiler: Compiler<State>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub(crate) struct CallSite {
    pub parent: Option<Pointer>,
}

impl CallSite {
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

    /// Runtime link-capability snapshot associated with this evaluator.
    pub fn runtime_env(&self) -> &RuntimeEnv {
        &self.runtime_env
    }

    /// Type system captured by the evaluator runtime.
    pub fn type_system(&self) -> Arc<TypeSystem> {
        Arc::clone(&self.runtime.type_system)
    }

    /// Heap used by this evaluator runtime.
    pub fn heap(&self) -> &Heap {
        &self.runtime.heap
    }

    /// Runtime capabilities available for satisfying compiled link contracts.
    pub fn capabilities(&self) -> &RuntimeCapabilities {
        self.runtime_env.capabilities()
    }

    /// Compare this runtime with a prepared program's link contract.
    pub fn compatibility_with(&self, program: &CompiledProgram) -> RuntimeCompatibility {
        self.runtime_env.compatibility_with(program)
    }

    /// Preflight a prepared program without consuming either value.
    pub fn validate(&self, program: &CompiledProgram) -> Result<(), EvalError> {
        self.runtime_env.validate(program)
    }

    /// Validate and run one prepared program, consuming the evaluator.
    pub async fn run(self, program: CompiledProgram) -> Result<Handle, EvalError> {
        self.runtime_env.validate_internal(&program)?;
        let runtime = self.runtime;
        let heap = runtime.heap.clone();
        let pointer = eval_typed_expr(runtime, program.env, program.expr)
            .await
            .map_err(EvalError::from)?;
        heap.handle(pointer).map_err(EvalError::from)
    }

    /// Compile and run a single expression, returning its value and inferred type.
    pub async fn eval(self, expr: &Expr) -> Result<(Handle, Type), ExecutionError> {
        let mut this = self;
        let program = this.compiler.compile_expr(expr)?;
        this.run_prepared(program).await
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

    /// Load a declaration-only module through an importer.
    pub async fn eval_module_with_importer(
        mut self,
        request: ImportRequest,
        importer: Arc<dyn Importer>,
    ) -> Result<(Handle, Type), ExecutionError> {
        let result: Result<(Handle, Type), ExecutionError> = {
            let engine = &mut self.compiler.engine;
            let chain = engine.modules.import_chain().with_importer(importer);
            let resolved = chain.import(request).await.map_err(CompileError::from)?;
            let inst = engine
                .load_module_from_resolved(resolved, &chain)
                .await
                .map_err(CompileError::from)?;
            Ok((inst.init_value, inst.init_type))
        };
        result
    }

    /// Load declaration-only module source directly.
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
                let chain = engine.modules.import_chain();
                let inst = engine
                    .load_module_from_resolved(
                        ResolvedModule {
                            id,
                            content: ResolvedModuleContent::Source(source.to_string()),
                        },
                        &chain,
                    )
                    .await
                    .map_err(CompileError::from)?;
                Ok((inst.init_value, inst.init_type))
            }
        };
        result
    }

    /// Compile and run a snippet, returning its value and inferred type.
    pub async fn eval_snippet(self, source: &str) -> Result<(Handle, Type), ExecutionError> {
        let mut this = self;
        let program = this.compiler.compile_snippet(source).await?;
        this.run_prepared(program).await
    }

    /// Compile and run a snippet using a path anchor for resolving relative imports.
    pub async fn eval_snippet_at(
        self,
        source: &str,
        importer_path: impl AsRef<Path>,
    ) -> Result<(Handle, Type), ExecutionError> {
        let mut this = self;
        let program = this
            .compiler
            .compile_snippet_at(source, importer_path.as_ref())
            .await?;
        this.run_prepared(program).await
    }
}

fn cell_type(heap: &HeapAccess<'_>, cell: &Cell) -> Result<Type, EngineError> {
    let pointer_type = |pointer: &Pointer| -> Result<Type, EngineError> {
        let cell = heap.get(pointer)?;
        cell_type(heap, cell)
    };

    match cell {
        Cell::Bool(..) => Ok(Type::builtin(BuiltinTypeId::Bool)),
        Cell::U8(..) => Ok(Type::builtin(BuiltinTypeId::U8)),
        Cell::U16(..) => Ok(Type::builtin(BuiltinTypeId::U16)),
        Cell::U32(..) => Ok(Type::builtin(BuiltinTypeId::U32)),
        Cell::U64(..) => Ok(Type::builtin(BuiltinTypeId::U64)),
        Cell::I8(..) => Ok(Type::builtin(BuiltinTypeId::I8)),
        Cell::I16(..) => Ok(Type::builtin(BuiltinTypeId::I16)),
        Cell::I32(..) => Ok(Type::builtin(BuiltinTypeId::I32)),
        Cell::I64(..) => Ok(Type::builtin(BuiltinTypeId::I64)),
        Cell::F32(..) => Ok(Type::builtin(BuiltinTypeId::F32)),
        Cell::F64(..) => Ok(Type::builtin(BuiltinTypeId::F64)),
        Cell::String(..) => Ok(Type::builtin(BuiltinTypeId::String)),
        Cell::Uuid(..) => Ok(Type::builtin(BuiltinTypeId::Uuid)),
        Cell::DateTime(..) => Ok(Type::builtin(BuiltinTypeId::DateTime)),
        Cell::Tuple(elems) => {
            let mut tys = Vec::with_capacity(elems.len());
            for elem in elems {
                tys.push(pointer_type(elem)?);
            }
            Ok(Type::tuple(tys))
        }
        Cell::Array(elems) => {
            let first = elems
                .first()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("array")))?;
            let elem_ty = pointer_type(first)?;
            for elem in elems.iter().skip(1) {
                let ty = pointer_type(elem)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::Array), elem_ty))
        }
        Cell::Dict(map) => {
            let first = map
                .values()
                .next()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("dict")))?;
            let elem_ty = pointer_type(first)?;
            for val in map.values().skip(1) {
                let ty = pointer_type(val)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::Dict), elem_ty))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Some" && args.len() == 1 => {
            let inner = pointer_type(&args[0])?;
            Ok(Type::app(Type::builtin(BuiltinTypeId::Option), inner))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "None" && args.is_empty() => {
            Err(EngineError::UnknownType(Symbol::intern("option")))
        }
        Cell::Adt(tag, args)
            if (tag.as_ref() == "Ok" || tag.as_ref() == "Err") && args.len() == 1 =>
        {
            Err(EngineError::UnknownType(Symbol::intern("result")))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Empty" && args.is_empty() => {
            Err(EngineError::UnknownType(Symbol::intern("list")))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
            let elem_ty = pointer_type(&args[0])?;
            Ok(Type::app(Type::builtin(BuiltinTypeId::List), elem_ty))
        }
        Cell::Adt(tag, _args) if tag.as_ref() == "Empty" || tag.as_ref() == "Cons" => {
            Err(EngineError::NativeType {
                expected: "list".into(),
                got: cell.cell_type_name().into(),
            })
        }
        Cell::Adt(tag, _args) => Err(EngineError::UnknownType(tag.clone())),
        Cell::Uninitialized(..) => Err(EngineError::UnknownType(Symbol::intern("uninitialized"))),
        Cell::Frame(..) => Err(EngineError::UnknownType(Symbol::intern("frame"))),
        Cell::Closure(..) => Err(EngineError::UnknownType(Symbol::intern("closure"))),
        Cell::Native(..) => Err(EngineError::UnknownType(Symbol::intern("native"))),
        Cell::Overloaded(..) => Err(EngineError::UnknownType(Symbol::intern("overloaded"))),
    }
}

pub(crate) fn resolve_arg_type(
    heap: &Heap,
    arg_type: Option<&Type>,
    arg: &Pointer,
) -> Result<Type, EngineError> {
    let infer_from_cell = |ty_hint: Option<&Type>| -> Result<Type, EngineError> {
        heap.with_access(|heap| {
            let cell = heap.get(arg)?;
            match ty_hint {
                Some(ty) => match cell_type(heap, cell) {
                    Ok(val_ty) if val_ty.ftv().is_empty() => Ok(val_ty),
                    _ => Ok(ty.clone()),
                },
                None => cell_type(heap, cell),
            }
        })
    };
    match arg_type {
        Some(ty) if ty.ftv().is_empty() => Ok(ty.clone()),
        Some(ty) => infer_from_cell(Some(ty)),
        None => infer_from_cell(None),
    }
}

pub(crate) fn application_result_type(
    func_type: &Type,
    arg_type: &Type,
) -> Result<Type, EngineError> {
    let (expected_arg, result) =
        split_fun(func_type).ok_or_else(|| EngineError::NotCallable(func_type.to_string()))?;
    let subst = unify(&expected_arg, arg_type).map_err(|_| EngineError::NativeType {
        expected: expected_arg.to_string(),
        got: arg_type.to_string(),
    })?;
    Ok(result.apply(&subst))
}
