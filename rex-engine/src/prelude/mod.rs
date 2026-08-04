//! Prelude injection helpers for Rex.

macro_rules! scheme_class_name {
    ($class:ident) => {
        stringify!($class)
    };
    ($class:literal) => {
        $class
    };
}

macro_rules! scheme_pred {
    ($class:tt($typ:expr)) => {
        Predicate::new(scheme_class_name!($class), $typ)
    };
    ($class:tt($first:expr, $($rest:expr),+ $(,)?)) => {
        Predicate::new(
            scheme_class_name!($class),
            Type::tuple(vec![Into::<Type>::into($first), $(Into::<Type>::into($rest)),+]),
        )
    };
}

macro_rules! scheme {
    ($typ:expr $(,)?) => {
        Scheme::new(vec![], vec![], $typ)
    };
    ($supply:expr; forall [$($v:ident),* $(,)?] => $typ:expr $(,)?) => {{
        let mut vars = Vec::new();
        $(
            let tv = ($supply).fresh(Some(Symbol::intern(stringify!($v))));
            let $v = &Type::var(tv.clone());
            vars.push(tv);
        )*
        Scheme::new(vars, vec![], $typ)
    }};
    ($supply:expr; forall [$($v:ident),* $(,)?]
        where [$($class:tt($($arg:expr),+)),* $(,)?]
        => $typ:expr $(,)?
    ) => {{
        let mut vars = Vec::new();
        $(
            let tv = ($supply).fresh(Some(Symbol::intern(stringify!($v))));
            let $v = &Type::var(tv.clone());
            vars.push(tv);
        )*
        Scheme::new(
            vars,
            vec![$(scheme_pred!($class($($arg),+))),*],
            $typ,
        )
    }};
}

mod type_system;

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use chrono::{DateTime, Utc};
use rex_ast::{CompilationUnit, Decl, Symbol};
use rex_parser::parse;
use rex_typesystem::{
    error::TypeError,
    types::{BuiltinTypeId, Scheme, Type, TypeKind, Types},
    typesystem::TypeSystem,
    unification::unify,
};
use uuid::Uuid;

use crate::{
    EngineError,
    builder::core::{Builder, StaticModuleImporter},
    evaluator::{
        native_callable::SchedulerNativeResult,
        native_functions::{
            NativeApplyUnary, NativeArrayEq, NativeArrayEqState, NativeDictMap, NativeDictTraverse,
            NativeFold, NativeFoldOrder, NativeFoldState, NativeMean, NativeMeanState,
            NativeSequenceFilter, NativeSequenceFilterMap, NativeSequenceFlatMap,
            NativeSequenceMap, NativeSequenceShape, NativeSum, NativeTask, NativeUnaryFilter,
            NativeUnaryFilterMap, NativeUnaryFlatMap, NativeUnaryMap,
        },
    },
    memory::{
        heap::{RootScope, RootedPtr},
        lists::ListItems,
    },
    modules::{
        CanonicalSymbol, CompilationPackage, Declarations, ModuleExports, ModuleId,
        PRELUDE_MODULE_NAME, ResolvedModule, ResolvedModuleContent, SymbolKind, VirtualModule,
        module_key_for_module,
    },
    stack::NativeUnaryShape,
    util::split_fun,
};

pub fn prelude_typeclasses_program() -> Result<&'static CompilationUnit, EngineError> {
    static PROGRAM: OnceLock<Result<CompilationUnit, String>> = OnceLock::new();
    let parsed = PROGRAM.get_or_init(|| {
        let source = include_str!("typeclasses.rex");
        match parse(source) {
            Ok(program) => Ok(program),
            Err(errs) => {
                let mut out = String::from("prelude typeclasses: parse error:");
                for err in errs {
                    out.push_str(&format!("\n  {err}"));
                }
                Err(out)
            }
        }
    });
    match parsed {
        Ok(program) => Ok(program),
        Err(msg) => Err(EngineError::Type(TypeError::Internal(msg.clone()))),
    }
}

pub fn standard_type_system() -> Result<TypeSystem, EngineError> {
    let program = prelude_typeclasses_program()?;
    let mut ts = TypeSystem::new();
    type_system::inject_standard_prelude(&mut ts, &program.decls)?;
    Ok(ts)
}

pub(crate) fn inject_prelude<State>(engine: &mut Builder<State>) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    inject_prelude_adts(engine)?;
    inject_equality_ops(engine)?;
    inject_order_ops(engine)?;
    inject_show_ops(engine)?;
    inject_boolean_ops(engine)?;
    inject_numeric_ops(engine)?;
    inject_list_builtins(engine)?;
    inject_option_result_builtins(engine)?;
    inject_json_primops(engine)?;
    register_prelude_typeclass_instances(engine)?;
    Ok(())
}

fn register_prelude_typeclass_instances<State>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    // The type system prelude injects the *heads* of the standard instances.
    // The evaluator also needs the *method bodies* so class method lookup can
    // produce actual values at runtime.
    let program = prelude_typeclasses_program()?;
    for decl in program.decls.iter() {
        let Decl::Instance(inst_decl) = decl else {
            continue;
        };
        if inst_decl.methods.is_empty() {
            continue;
        }
        let prepared = engine
            .type_system
            .prepare_instance_decl(inst_decl)
            .map_err(EngineError::Type)?;
        engine.register_typeclass_instance(inst_decl, &prepared)?;
    }
    Ok(())
}

pub(crate) fn inject_prelude_virtual_module<State>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if engine
        .module_loader
        .virtual_modules
        .contains_key(PRELUDE_MODULE_NAME)
    {
        return Ok(());
    }

    let prelude_module_id = ModuleId::parse(PRELUDE_MODULE_NAME)?;
    let module_key = module_key_for_module(&prelude_module_id);
    let mut exports = ModuleExports::default();
    for (name, _) in engine.type_system.env.values.iter() {
        if !name.as_ref().starts_with("@m") {
            exports.insert_value(
                name.clone(),
                CanonicalSymbol::from_symbol(
                    module_key,
                    SymbolKind::Value,
                    name.clone(),
                    name.clone(),
                ),
            );
        }
    }

    for name in engine.type_system.adts.keys() {
        if !name.as_ref().starts_with("@m") {
            exports.insert_type(
                name.clone(),
                CanonicalSymbol::from_symbol(
                    module_key,
                    SymbolKind::Type,
                    name.clone(),
                    name.clone(),
                ),
            );
        }
    }

    for name in engine.type_system.class_info.keys() {
        if !name.as_ref().starts_with("@m") {
            exports.insert_class(
                name.clone(),
                CanonicalSymbol::from_symbol(
                    module_key,
                    SymbolKind::Class,
                    name.clone(),
                    name.clone(),
                ),
            );
        }
    }

    let module_id = prelude_module_id;
    let compilation_unit = CompilationUnit {
        decls: Vec::new(),
        body: None,
    };
    engine
        .module_loader
        .module_exports_cache
        .insert(module_id.clone(), exports.clone());
    engine
        .module_loader
        .module_interface_cache
        .insert(module_id.clone(), Declarations::default());
    engine.module_loader.virtual_modules.insert(
        PRELUDE_MODULE_NAME.to_string(),
        VirtualModule {
            package: CompilationPackage::from(&compilation_unit),
        },
    );
    engine
        .module_loader
        .system
        .prepend_importer(Arc::new(StaticModuleImporter {
            module_id: module_id.clone(),
            resolved: ResolvedModule {
                id: module_id,
                content: ResolvedModuleContent::CompilationPackage(CompilationPackage::from(
                    &compilation_unit,
                )),
            },
        }));
    Ok(())
}

fn integer_overflow(typ: &'static str) -> EngineError {
    EngineError::from(format!("integer overflow ({typ})"))
}

fn integer_underflow(typ: &'static str) -> EngineError {
    EngineError::from(format!("integer underflow ({typ})"))
}

fn checked_integer_error(value: i128, min: i128, max: i128, typ: &'static str) -> EngineError {
    if value < min {
        integer_underflow(typ)
    } else {
        debug_assert!(value > max);
        integer_overflow(typ)
    }
}

fn list_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::List)) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            expected: "List a".into(),
            got: typ.to_string(),
        }),
    }
}

fn dict_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Dict)) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            expected: "Dict a".into(),
            got: typ.to_string(),
        }),
    }
}

fn option_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Option)) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            expected: "Option a".into(),
            got: typ.to_string(),
        }),
    }
}

fn result_types(typ: &Type) -> Result<(Type, Type), EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, ok) => match head.as_ref() {
            TypeKind::App(head, err) if matches!(head.as_ref(), TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Result)) => {
                Ok((ok.clone(), err.clone()))
            }
            _ => Err(EngineError::NativeType {
                expected: "Result a e".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            expected: "Result a e".into(),
            got: typ.to_string(),
        }),
    }
}

fn option_from_root(
    scope: &mut RootScope<'_>,
    value: Option<RootedPtr>,
) -> Result<RootedPtr, EngineError> {
    match value {
        Some(v) => scope.alloc_root_adt(Symbol::intern("Some"), vec![v]),
        None => scope.alloc_root_adt(Symbol::intern("None"), vec![]),
    }
}

fn option_value(
    scope: &mut RootScope<'_>,
    root: RootedPtr,
) -> Result<Option<RootedPtr>, EngineError> {
    let (tag, args) = scope.root_as_adt(root)?;
    if tag.as_ref() == "Some" && args.len() == 1 {
        Ok(Some(args[0]))
    } else if tag.as_ref() == "None" && args.is_empty() {
        Ok(None)
    } else {
        Err(EngineError::NativeType {
            expected: "Option".into(),
            got: scope.type_name(root)?.into(),
        })
    }
}

fn result_value(
    scope: &mut RootScope<'_>,
    root: RootedPtr,
) -> Result<Result<RootedPtr, RootedPtr>, EngineError> {
    let (tag, args) = scope.root_as_adt(root)?;
    if tag.as_ref() == "Ok" && args.len() == 1 {
        Ok(Ok(args[0]))
    } else if tag.as_ref() == "Err" && args.len() == 1 {
        Ok(Err(args[0]))
    } else {
        Err(EngineError::NativeType {
            expected: "Result".into(),
            got: scope.type_name(root)?.into(),
        })
    }
}

fn result_from_root(
    scope: &mut RootScope<'_>,
    value: Result<RootedPtr, RootedPtr>,
) -> Result<RootedPtr, EngineError> {
    match value {
        Ok(v) => scope.alloc_root_adt(Symbol::intern("Ok"), vec![v]),
        Err(v) => scope.alloc_root_adt(Symbol::intern("Err"), vec![v]),
    }
}

fn split_fun_chain(typ: &Type, count: usize) -> Result<(Vec<Type>, Type), EngineError> {
    let mut args = Vec::with_capacity(count);
    let mut cur = typ.clone();
    for _ in 0..count {
        let (arg, rest) = match cur.as_ref() {
            TypeKind::Fun(arg, rest) => (arg.clone(), rest.clone()),
            _ => {
                return Err(EngineError::NativeType {
                    expected: format!("function of arity {}", count),
                    got: typ.to_string(),
                });
            }
        };
        args.push(arg);
        cur = rest;
    }
    Ok((args, cur))
}

fn tuple_elem_type(typ: &Type) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::Tuple(elems) if !elems.is_empty() => {
            let first = elems[0].clone();
            for elem in elems.iter().skip(1) {
                if *elem != first {
                    return Err(EngineError::NativeType {
                        expected: first.to_string(),
                        got: elem.to_string(),
                    });
                }
            }
            Ok(first)
        }
        _ => Err(EngineError::NativeType {
            expected: "tuple".into(),
            got: typ.to_string(),
        }),
    }
}

fn extremum_root_by_type(
    scope: &mut RootScope<'_>,
    elem_ty: &Type,
    values: Vec<RootedPtr>,
    choose: std::cmp::Ordering,
) -> Result<RootedPtr, EngineError> {
    let mut values = values.into_iter();
    let mut best = values.next().ok_or(EngineError::EmptySequence)?;
    for value in values {
        let ord = cmp_rooted_by_type(scope, elem_ty, value, best)?;
        if ord == choose {
            best = value;
        }
    }
    Ok(best)
}

fn checked_index(name: Symbol, index: i32, len: usize) -> Result<usize, EngineError> {
    if index < 0 {
        return Err(EngineError::IndexOutOfBounds { name, index, len });
    }
    let index_usize = index as usize;
    if index_usize >= len {
        return Err(EngineError::IndexOutOfBounds { name, index, len });
    }
    Ok(index_usize)
}

fn checked_endpoint(name: Symbol, index: i32, len: usize) -> Result<usize, EngineError> {
    if index < 0 {
        return Err(EngineError::IndexOutOfBounds { name, index, len });
    }
    let index_usize = index as usize;
    if index_usize > len {
        return Err(EngineError::IndexOutOfBounds { name, index, len });
    }
    Ok(index_usize)
}

fn list_range_from_items(
    scope: &mut RootScope<'_>,
    items: Vec<RootedPtr>,
    start: usize,
    end: usize,
) -> Result<RootedPtr, EngineError> {
    if start > end || end > items.len() {
        return Err(EngineError::Internal(format!(
            "invalid list item range {start}..{end} for len {}",
            items.len()
        )));
    }
    scope.alloc_root_list(items[start..end].to_vec())
}

fn zip_tuple2_roots(
    scope: &mut RootScope<'_>,
    xs: Vec<RootedPtr>,
    ys: Vec<RootedPtr>,
) -> Result<Vec<RootedPtr>, EngineError> {
    let mut pairs = Vec::with_capacity(xs.len().min(ys.len()));
    for (left, right) in xs.into_iter().zip(ys) {
        pairs.push(scope.alloc_root_tuple(vec![left, right])?);
    }
    Ok(pairs)
}

fn unzip_tuple2_roots(
    scope: &mut RootScope<'_>,
    pairs: Vec<RootedPtr>,
) -> Result<(Vec<RootedPtr>, Vec<RootedPtr>), EngineError> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for pair in pairs {
        let elems = scope.root_as_tuple(pair)?;
        let len = elems.len();
        if len != 2 {
            return Err(EngineError::NativeType {
                expected: "tuple2".into(),
                got: format!("tuple{len}"),
            });
        }
        left.push(elems[0]);
        right.push(elems[1]);
    }
    Ok((left, right))
}

fn as_nonneg_usize(n: i32) -> usize {
    if n <= 0 { 0 } else { n as usize }
}

fn cmp_rooted_by_type(
    scope: &RootScope<'_>,
    typ: &Type,
    lhs: RootedPtr,
    rhs: RootedPtr,
) -> Result<std::cmp::Ordering, EngineError> {
    let mismatch = |expected: &str| EngineError::NativeType {
        expected: expected.to_string(),
        got: format!(
            "{}, {}",
            scope.type_name(lhs).unwrap_or("<invalid>"),
            scope.type_name(rhs).unwrap_or("<invalid>")
        ),
    };

    macro_rules! compare_ord {
        ($accessor:ident, $expected:expr) => {{
            let lhs = scope.$accessor(lhs).map_err(|_| mismatch($expected))?;
            let rhs = scope.$accessor(rhs).map_err(|_| mismatch($expected))?;
            Ok(lhs.cmp(&rhs))
        }};
    }

    match typ.as_ref() {
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::U8) => compare_ord!(root_as_u8, tc.name_str()),
            Some(BuiltinTypeId::U16) => compare_ord!(root_as_u16, tc.name_str()),
            Some(BuiltinTypeId::U32) => compare_ord!(root_as_u32, tc.name_str()),
            Some(BuiltinTypeId::U64) => compare_ord!(root_as_u64, tc.name_str()),
            Some(BuiltinTypeId::I8) => compare_ord!(root_as_i8, tc.name_str()),
            Some(BuiltinTypeId::I16) => compare_ord!(root_as_i16, tc.name_str()),
            Some(BuiltinTypeId::I32) => compare_ord!(root_as_i32, tc.name_str()),
            Some(BuiltinTypeId::I64) => compare_ord!(root_as_i64, tc.name_str()),
            Some(BuiltinTypeId::F32) => {
                let lhs = scope
                    .root_as_f32(lhs)
                    .map_err(|_| mismatch(tc.name_str()))?;
                let rhs = scope
                    .root_as_f32(rhs)
                    .map_err(|_| mismatch(tc.name_str()))?;
                lhs.partial_cmp(&rhs)
                    .ok_or_else(|| EngineError::NativeType {
                        expected: tc.name_str().to_string(),
                        got: "nan".into(),
                    })
            }
            Some(BuiltinTypeId::F64) => {
                let lhs = scope
                    .root_as_f64(lhs)
                    .map_err(|_| mismatch(tc.name_str()))?;
                let rhs = scope
                    .root_as_f64(rhs)
                    .map_err(|_| mismatch(tc.name_str()))?;
                lhs.partial_cmp(&rhs)
                    .ok_or_else(|| EngineError::NativeType {
                        expected: tc.name_str().to_string(),
                        got: "nan".into(),
                    })
            }
            Some(BuiltinTypeId::String) => compare_ord!(root_as_string, tc.name_str()),
            Some(BuiltinTypeId::Uuid) => compare_ord!(root_as_uuid, tc.name_str()),
            Some(BuiltinTypeId::DateTime) => compare_ord!(root_as_datetime, tc.name_str()),
            _ => Err(mismatch(tc.name_str())),
        },
        _ => Err(mismatch(&typ.to_string())),
    }
}

fn inject_prelude_adts<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    let mut list_adt = engine.adt_decl("List", &["a"]);
    let a_name = Symbol::intern("a");
    let a = list_adt
        .param_type(&a_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("List")))?;
    let list_a = list_adt.result_type();
    list_adt.add_variant(Symbol::intern("Empty"), vec![]);
    list_adt.add_variant(Symbol::intern("Cons"), vec![a, list_a]);
    engine.type_system.register_adt(&list_adt);
    for (ctor, scheme) in list_adt.constructor_schemes() {
        match ctor.as_ref() {
            "Empty" => {
                engine.export_native(ctor.to_string(), scheme, 0, |scope, _, args| {
                    if !args.is_empty() {
                        return Err(EngineError::Internal(
                            "Empty constructor received arguments".into(),
                        ));
                    }
                    scope.alloc_root_empty()
                })?;
            }
            "Cons" => {
                engine.export_native(ctor.to_string(), scheme, 2, |scope, _, args| {
                    let [head, tail] = args else {
                        return Err(EngineError::Internal(
                            "Cons constructor received wrong arity".into(),
                        ));
                    };
                    scope.alloc_root_cons(*head, *tail)
                })?;
            }
            _ => {}
        }
    }

    let mut option_adt = engine.adt_decl("Option", &["t"]);
    let t_name = Symbol::intern("t");
    let t = option_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("Option")))?;
    option_adt.add_variant(Symbol::intern("Some"), vec![t]);
    option_adt.add_variant(Symbol::intern("None"), vec![]);
    engine.inject_adt(option_adt)?;

    let mut result_adt = engine.adt_decl("Result", &["e", "t"]);
    let e_name = Symbol::intern("e");
    let t_name = Symbol::intern("t");
    let e = result_adt
        .param_type(&e_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("Result")))?;
    let t = result_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("Result")))?;
    result_adt.add_variant(Symbol::intern("Err"), vec![e]);
    result_adt.add_variant(Symbol::intern("Ok"), vec![t]);
    engine.inject_adt(result_adt)?;
    Ok(())
}

fn inject_equality_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    // Equality primitives are monomorphic overloads (same name, different
    // concrete types), matching the numeric `prim_add` style.
    engine.export("prim_eq", |_: &State, a: bool, b: bool| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: bool, b: bool| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: u8, b: u8| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u8, b: u8| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: u16, b: u16| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u16, b: u16| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: u32, b: u32| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u32, b: u32| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: u64, b: u64| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: u64, b: u64| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: i8, b: i8| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i8, b: i8| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: i16, b: i16| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i16, b: i16| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: i32, b: i32| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i32, b: i32| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: i64, b: i64| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: i64, b: i64| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: f32, b: f32| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: f32, b: f32| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: f64, b: f64| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: f64, b: f64| Ok(a != b))?;

    engine.export("prim_eq", |_: &State, a: String, b: String| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: String, b: String| Ok(a != b))?;
    engine.export("prim_eq", |_: &State, a: Uuid, b: Uuid| Ok(a == b))?;
    engine.export("prim_ne", |_: &State, a: Uuid, b: Uuid| Ok(a != b))?;
    engine.export(
        "prim_eq",
        |_: &State, a: DateTime<Utc>, b: DateTime<Utc>| Ok(a == b),
    )?;
    engine.export(
        "prim_ne",
        |_: &State, a: DateTime<Utc>, b: DateTime<Utc>| Ok(a != b),
    )?;

    // List equality must respect `Eq a`. We can't express the loop without a
    // primitive, but we *can* express the element comparison: the primitive
    // calls `(==)` on each pair.
    {
        let bool_ty = Type::builtin(BuiltinTypeId::Bool);
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::list(a), Type::fun(Type::list(a), &bool_ty))
        );
        engine.export_native_scheduler(
            "prim_list_eq",
            scheme.clone(),
            2,
            |scope, call_type, args| {
                let (lhs_ty, rhs_ty) = binary_arg_types(&call_type)?;
                let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
                    expected: lhs_ty.to_string(),
                    got: rhs_ty.to_string(),
                })?;
                let list_ty = lhs_ty.apply(&subst);
                let elem_ty = list_elem_type(&list_ty)?;
                let xs = scope.list_items(args[0])?;
                let ys = scope.list_items(args[1])?;
                Ok(SchedulerNativeResult::Task(NativeTask::ArrayEq(
                    NativeArrayEq {
                        elem_type: elem_ty,
                        xs,
                        ys,
                        state: NativeArrayEqState::Enter,
                        next_index: 0,
                        step: None,
                        negate: false,
                    },
                )))
            },
        )?;

        engine.export_native_scheduler("prim_list_ne", scheme, 2, |scope, call_type, args| {
            let (lhs_ty, rhs_ty) = binary_arg_types(&call_type)?;
            let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
                expected: lhs_ty.to_string(),
                got: rhs_ty.to_string(),
            })?;
            let list_ty = lhs_ty.apply(&subst);
            let elem_ty = list_elem_type(&list_ty)?;
            let xs = scope.list_items(args[0])?;
            let ys = scope.list_items(args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::ArrayEq(
                NativeArrayEq {
                    elem_type: elem_ty,
                    xs,
                    ys,
                    state: NativeArrayEqState::Enter,
                    next_index: 0,
                    step: None,
                    negate: true,
                },
            )))
        })?;
    }

    Ok(())
}

fn inject_order_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    fn cmp_to_i32(ord: std::cmp::Ordering) -> i32 {
        match ord {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    // Integer and string comparisons can be injected as direct typed natives,
    // with no runtime type switching.
    engine.export("prim_lt", |_: &State, a: u8, b: u8| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u8, b: u8| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u8, b: u8| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u8, b: u8| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u8, b: u8| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: u16, b: u16| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u16, b: u16| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u16, b: u16| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u16, b: u16| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u16, b: u16| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: u32, b: u32| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u32, b: u32| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u32, b: u32| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u32, b: u32| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u32, b: u32| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: u64, b: u64| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: u64, b: u64| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: u64, b: u64| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: u64, b: u64| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: u64, b: u64| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i8, b: i8| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i8, b: i8| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i8, b: i8| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i8, b: i8| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i8, b: i8| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i16, b: i16| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i16, b: i16| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i16, b: i16| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i16, b: i16| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i16, b: i16| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i32, b: i32| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i32, b: i32| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i32, b: i32| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i32, b: i32| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i32, b: i32| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: i64, b: i64| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: i64, b: i64| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: i64, b: i64| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: i64, b: i64| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: i64, b: i64| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    engine.export("prim_lt", |_: &State, a: String, b: String| Ok(a < b))?;
    engine.export("prim_le", |_: &State, a: String, b: String| Ok(a <= b))?;
    engine.export("prim_gt", |_: &State, a: String, b: String| Ok(a > b))?;
    engine.export("prim_ge", |_: &State, a: String, b: String| Ok(a >= b))?;
    engine.export("prim_cmp", |_: &State, a: String, b: String| {
        Ok(cmp_to_i32(a.cmp(&b)))
    })?;

    // Floats: preserve the existing “NaN is a type error” semantics.
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let i32_ty = Type::builtin(BuiltinTypeId::I32);

    let f32_ty = Type::builtin(BuiltinTypeId::F32);
    let f32_bool = scheme!(Type::fun(&f32_ty, Type::fun(&f32_ty, &bool_ty)));
    let f32_cmp = scheme!(Type::fun(&f32_ty, Type::fun(&f32_ty, &i32_ty)));
    for (name, pred) in [
        (
            "prim_lt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_le",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_gt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_ge",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
    ] {
        let scheme = f32_bool.clone();
        engine.export_native(name, scheme, 2, move |scope, _, args| {
            let a = scope.root_as_f32(args[0])?;
            let b = scope.root_as_f32(args[1])?;
            let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                expected: "f32".into(),
                got: "nan".into(),
            })?;
            scope.alloc_root_bool(pred(ord))
        })?;
    }
    engine.export_native("prim_cmp", f32_cmp, 2, |scope, _, args| {
        let a = scope.root_as_f32(args[0])?;
        let b = scope.root_as_f32(args[1])?;
        let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
            expected: "f32".into(),
            got: "nan".into(),
        })?;
        scope.alloc_root_i32(cmp_to_i32(ord))
    })?;

    let f64_ty = Type::builtin(BuiltinTypeId::F64);
    let f64_bool = scheme!(Type::fun(&f64_ty, Type::fun(&f64_ty, &bool_ty)));
    let f64_cmp = scheme!(Type::fun(&f64_ty, Type::fun(&f64_ty, &i32_ty)));
    for (name, pred) in [
        (
            "prim_lt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_le",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_gt",
            (|o: std::cmp::Ordering| o == std::cmp::Ordering::Greater)
                as fn(std::cmp::Ordering) -> bool,
        ),
        (
            "prim_ge",
            (|o: std::cmp::Ordering| o != std::cmp::Ordering::Less)
                as fn(std::cmp::Ordering) -> bool,
        ),
    ] {
        let scheme = f64_bool.clone();
        engine.export_native(name, scheme, 2, move |scope, _, args| {
            let a = scope.root_as_f64(args[0])?;
            let b = scope.root_as_f64(args[1])?;
            let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
                expected: "f64".into(),
                got: "nan".into(),
            })?;
            scope.alloc_root_bool(pred(ord))
        })?;
    }
    engine.export_native("prim_cmp", f64_cmp, 2, |scope, _, args| {
        let a = scope.root_as_f64(args[0])?;
        let b = scope.root_as_f64(args[1])?;
        let ord = a.partial_cmp(&b).ok_or_else(|| EngineError::NativeType {
            expected: "f64".into(),
            got: "nan".into(),
        })?;
        scope.alloc_root_i32(cmp_to_i32(ord))
    })?;

    Ok(())
}

fn inject_show_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    engine.export("prim_show", |_: &State, x: bool| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u8| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u16| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u32| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: u64| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i8| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i16| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i32| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: i64| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: f32| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: f64| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: String| Ok(x))?;
    engine.export("prim_show", |_: &State, x: Uuid| Ok(x.to_string()))?;
    engine.export("prim_show", |_: &State, x: DateTime<Utc>| Ok(x.to_string()))?;
    Ok(())
}

fn inject_boolean_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    engine.export("(&&)", |_: &State, a: bool, b: bool| Ok(a && b))?;
    engine.export("(||)", |_: &State, a: bool, b: bool| Ok(a || b))?;
    Ok(())
}

fn inject_numeric_ops<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    macro_rules! export_checked_unsigned_add {
        ($ty:ty) => {
            engine.export("prim_add", |_: &State, a: $ty, b: $ty| {
                a.checked_add(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_signed_add {
        ($ty:ty) => {
            engine.export("prim_add", |_: &State, a: $ty, b: $ty| {
                a.checked_add(b).ok_or_else(|| {
                    checked_integer_error(
                        a as i128 + b as i128,
                        <$ty>::MIN as i128,
                        <$ty>::MAX as i128,
                        stringify!($ty),
                    )
                })
            })?;
        };
    }
    macro_rules! export_checked_unsigned_sub {
        ($ty:ty) => {
            engine.export("prim_sub", |_: &State, a: $ty, b: $ty| {
                a.checked_sub(b)
                    .ok_or_else(|| integer_underflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_signed_sub {
        ($ty:ty) => {
            engine.export("prim_sub", |_: &State, a: $ty, b: $ty| {
                a.checked_sub(b).ok_or_else(|| {
                    checked_integer_error(
                        a as i128 - b as i128,
                        <$ty>::MIN as i128,
                        <$ty>::MAX as i128,
                        stringify!($ty),
                    )
                })
            })?;
        };
    }
    macro_rules! export_checked_unsigned_mul {
        ($ty:ty) => {
            engine.export("prim_mul", |_: &State, a: $ty, b: $ty| {
                a.checked_mul(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_signed_mul {
        ($ty:ty) => {
            engine.export("prim_mul", |_: &State, a: $ty, b: $ty| {
                a.checked_mul(b).ok_or_else(|| {
                    checked_integer_error(
                        a as i128 * b as i128,
                        <$ty>::MIN as i128,
                        <$ty>::MAX as i128,
                        stringify!($ty),
                    )
                })
            })?;
        };
    }
    macro_rules! export_checked_int_div {
        ($ty:ty) => {
            engine.export("prim_div", |_: &State, a: $ty, b: $ty| {
                a.checked_div(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }
    macro_rules! export_checked_int_rem {
        ($ty:ty) => {
            engine.export("prim_mod", |_: &State, a: $ty, b: $ty| {
                a.checked_rem(b)
                    .ok_or_else(|| integer_overflow(stringify!($ty)))
            })?;
        };
    }

    // Additive identity
    engine.export_value("prim_zero", String::new())?;
    engine.export_value("prim_zero", 0u8)?;
    engine.export_value("prim_zero", 0u16)?;
    engine.export_value("prim_zero", 0u32)?;
    engine.export_value("prim_zero", 0u64)?;
    engine.export_value("prim_zero", 0i8)?;
    engine.export_value("prim_zero", 0i16)?;
    engine.export_value("prim_zero", 0i32)?;
    engine.export_value("prim_zero", 0i64)?;
    engine.export_value("prim_zero", 0.0f32)?;
    engine.export_value("prim_zero", 0.0f64)?;

    // Multiplicative identity
    engine.export_value("prim_one", 1u8)?;
    engine.export_value("prim_one", 1u16)?;
    engine.export_value("prim_one", 1u32)?;
    engine.export_value("prim_one", 1u64)?;
    engine.export_value("prim_one", 1i8)?;
    engine.export_value("prim_one", 1i16)?;
    engine.export_value("prim_one", 1i32)?;
    engine.export_value("prim_one", 1i64)?;
    engine.export_value("prim_one", 1.0f32)?;
    engine.export_value("prim_one", 1.0f64)?;

    // Addition
    export_checked_unsigned_add!(u8);
    export_checked_unsigned_add!(u16);
    export_checked_unsigned_add!(u32);
    export_checked_unsigned_add!(u64);
    export_checked_signed_add!(i8);
    export_checked_signed_add!(i16);
    export_checked_signed_add!(i32);
    export_checked_signed_add!(i64);
    engine.export("prim_add", |_: &State, a: f32, b: f32| Ok(a + b))?;
    engine.export("prim_add", |_: &State, a: f64, b: f64| Ok(a + b))?;
    engine.export("prim_add", |_: &State, a: String, b: String| {
        Ok(format!("{}{}", a, b))
    })?;

    // Subtraction and negation
    export_checked_unsigned_sub!(u8);
    export_checked_unsigned_sub!(u16);
    export_checked_unsigned_sub!(u32);
    export_checked_unsigned_sub!(u64);
    export_checked_signed_sub!(i8);
    export_checked_signed_sub!(i16);
    export_checked_signed_sub!(i32);
    export_checked_signed_sub!(i64);
    engine.export("prim_sub", |_: &State, a: f32, b: f32| Ok(a - b))?;
    engine.export("prim_sub", |_: &State, a: f64, b: f64| Ok(a - b))?;
    engine.export("prim_negate", |_: &State, a: i8| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: i16| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: i32| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: i64| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: f32| Ok(-a))?;
    engine.export("prim_negate", |_: &State, a: f64| Ok(-a))?;

    // Multiplication and division
    export_checked_unsigned_mul!(u8);
    export_checked_unsigned_mul!(u16);
    export_checked_unsigned_mul!(u32);
    export_checked_unsigned_mul!(u64);
    export_checked_signed_mul!(i8);
    export_checked_signed_mul!(i16);
    export_checked_signed_mul!(i32);
    export_checked_signed_mul!(i64);
    engine.export("prim_mul", |_: &State, a: f32, b: f32| Ok(a * b))?;
    engine.export("prim_mul", |_: &State, a: f64, b: f64| Ok(a * b))?;
    export_checked_int_div!(u8);
    export_checked_int_div!(u16);
    export_checked_int_div!(u32);
    export_checked_int_div!(u64);
    export_checked_int_div!(i8);
    export_checked_int_div!(i16);
    export_checked_int_div!(i32);
    export_checked_int_div!(i64);
    engine.export("prim_div", |_: &State, a: f32, b: f32| Ok(a / b))?;
    engine.export("prim_div", |_: &State, a: f64, b: f64| Ok(a / b))?;

    // Remainder
    export_checked_int_rem!(u8);
    export_checked_int_rem!(u16);
    export_checked_int_rem!(u32);
    export_checked_int_rem!(u64);
    export_checked_int_rem!(i8);
    export_checked_int_rem!(i16);
    export_checked_int_rem!(i32);
    export_checked_int_rem!(i64);

    // Numeric conversions (used by `std.json`).
    engine.export("prim_to_f64", |_: &State, x: u8| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: u16| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: u32| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: u64| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i8| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i16| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i32| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: i64| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: f32| Ok(x as f64))?;
    engine.export("prim_to_f64", |_: &State, x: f64| Ok(x))?;

    macro_rules! export_int_widen {
        ($src:ty => $dst:ty) => {
            engine.export("prim_widen_int", |_: &State, x: $src| Ok(x as $dst))?;
        };
    }

    export_int_widen!(i8 => i16);
    export_int_widen!(i8 => i32);
    export_int_widen!(i8 => i64);
    export_int_widen!(i16 => i32);
    export_int_widen!(i16 => i64);
    export_int_widen!(i32 => i64);
    export_int_widen!(u8 => u16);
    export_int_widen!(u8 => u32);
    export_int_widen!(u8 => u64);
    export_int_widen!(u8 => i16);
    export_int_widen!(u8 => i32);
    export_int_widen!(u8 => i64);
    export_int_widen!(u16 => u32);
    export_int_widen!(u16 => u64);
    export_int_widen!(u16 => i32);
    export_int_widen!(u16 => i64);
    export_int_widen!(u32 => u64);
    export_int_widen!(u32 => i64);

    // f64 -> Option <number> conversions (used by `std.json`).
    // - reject NaN/±inf
    // - for integer types: require integral `x` (fract == 0) and in range
    {
        macro_rules! inject_f64_to {
            ($name:literal, $dst_ty:expr, $convert:expr) => {{
                let scheme = scheme!(Type::fun(
                    Type::builtin(BuiltinTypeId::F64),
                    Type::option($dst_ty),
                ));
                engine.export_native($name, scheme, 1, move |scope, _t, args| {
                    let x = scope.root_as_f64(args[0])?;
                    let converted = $convert(scope, x)?;
                    option_from_root(scope, converted)
                })?;
            }};
        }

        inject_f64_to!(
            "prim_f64_to_u8",
            Type::builtin(BuiltinTypeId::U8),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u8::MIN as f64 && x <= u8::MAX as f64 {
                    Ok(Some(scope.alloc_root_u8(x as u8)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_u16",
            Type::builtin(BuiltinTypeId::U16),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u16::MIN as f64 && x <= u16::MAX as f64
                {
                    Ok(Some(scope.alloc_root_u16(x as u16)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_u32",
            Type::builtin(BuiltinTypeId::U32),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u32::MIN as f64 && x <= u32::MAX as f64
                {
                    Ok(Some(scope.alloc_root_u32(x as u32)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_u64",
            Type::builtin(BuiltinTypeId::U64),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= u64::MIN as f64 && x <= u64::MAX as f64
                {
                    Ok(Some(scope.alloc_root_u64(x as u64)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i8",
            Type::builtin(BuiltinTypeId::I8),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i8::MIN as f64 && x <= i8::MAX as f64 {
                    Ok(Some(scope.alloc_root_i8(x as i8)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i16",
            Type::builtin(BuiltinTypeId::I16),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i16::MIN as f64 && x <= i16::MAX as f64
                {
                    Ok(Some(scope.alloc_root_i16(x as i16)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i32",
            Type::builtin(BuiltinTypeId::I32),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i32::MIN as f64 && x <= i32::MAX as f64
                {
                    Ok(Some(scope.alloc_root_i32(x as i32)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_i64",
            Type::builtin(BuiltinTypeId::I64),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x.fract() == 0.0 && x >= i64::MIN as f64 && x <= i64::MAX as f64
                {
                    Ok(Some(scope.alloc_root_i64(x as i64)?))
                } else {
                    Ok(None)
                }
            }
        );
        inject_f64_to!(
            "prim_f64_to_f32",
            Type::builtin(BuiltinTypeId::F32),
            |scope: &mut RootScope<'_>, x: f64| -> Result<Option<RootedPtr>, EngineError> {
                if x.is_finite() && x >= f32::MIN as f64 && x <= f32::MAX as f64 {
                    Ok(Some(scope.alloc_root_f32(x as f32)?))
                } else {
                    Ok(None)
                }
            }
        );
    }

    Ok(())
}

fn inject_json_primops<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    // Dict mapping and traversal helpers (used by `std.json`).
    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::dict(a), Type::builtin(BuiltinTypeId::I32))
        );
        engine.export_native("prim_dict_length", scheme, 1, |scope, _, args| {
            let values = scope.root_as_dict(args[0])?;
            let length = i32::try_from(values.len())
                .map_err(|_| EngineError::Internal("dictionary length overflow".into()))?;
            scope.alloc_root_i32(length)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, b),
                Type::fun(Type::dict(a), Type::dict(b)),
            )
        );
        engine.export_native_scheduler("prim_dict_map", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let dict_ty = arg_tys[1].clone();
            let elem_ty = dict_elem_type(&dict_ty)?;
            let map = scope.root_as_dict(args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::DictMap(
                NativeDictMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    entries: map.into_iter().collect(),
                    children: Vec::new(),
                    output: BTreeMap::new(),
                    remaining: 0,
                },
            )))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b, e] =>
            Type::fun(
                Type::fun(a, Type::result(b, e)),
                Type::fun(Type::dict(a), Type::result(Type::dict(b), e)),
            )
        );
        engine.export_native_scheduler(
            "prim_dict_traverse_result",
            scheme,
            2,
            |scope, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let dict_ty = arg_tys[1].clone();
                let elem_ty = dict_elem_type(&dict_ty)?;
                let map = scope.root_as_dict(args[1])?;
                Ok(SchedulerNativeResult::Task(NativeTask::DictTraverse(
                    NativeDictTraverse {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        entries: map.into_iter().collect(),
                        next_index: 0,
                        output: BTreeMap::new(),
                    },
                )))
            },
        )?;
    }

    // Parsing helpers used by `std.json` instances.
    {
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let uuid_ty = Type::builtin(BuiltinTypeId::Uuid);
        let scheme = scheme!(Type::fun(&string_ty, Type::option(uuid_ty)));
        engine.export_native("prim_parse_uuid", scheme, 1, |scope, _, args| {
            let s = scope.root_as_string(args[0])?;
            let parsed = Uuid::parse_str(&s)
                .ok()
                .map(|uuid| scope.alloc_root_uuid(uuid))
                .transpose()?;
            option_from_root(scope, parsed)
        })?;
    }

    {
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let dt_ty = Type::builtin(BuiltinTypeId::DateTime);
        let scheme = scheme!(Type::fun(&string_ty, Type::option(dt_ty)));
        engine.export_native("prim_parse_datetime", scheme, 1, |scope, _, args| {
            let s = scope.root_as_string(args[0])?;
            let parsed = DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .map(|dt| scope.alloc_root_datetime(dt))
                .transpose()?;
            option_from_root(scope, parsed)
        })?;
    }

    // prim_json_stringify : a -> string
    //
    // Used by `std.json` to implement `Show Value` (JSON-encoded string).
    {
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(a, &string_ty)
        );

        #[derive(Clone)]
        struct Tags {
            null: Symbol,
            bool_: Symbol,
            string: Symbol,
            number: Symbol,
            array: Symbol,
            object: Symbol,
        }

        let tags = Tags {
            null: Symbol::intern("Null"),
            bool_: Symbol::intern("Bool"),
            string: Symbol::intern("String"),
            number: Symbol::intern("Number"),
            array: Symbol::intern("Array"),
            object: Symbol::intern("Object"),
        };

        fn to_serde_json(
            scope: &mut RootScope<'_>,
            value: RootedPtr,
            tags: &Tags,
        ) -> Option<serde_json::Value> {
            let (tag, args) = scope.root_as_adt(value).ok()?;
            if tag == tags.null {
                return Some(serde_json::Value::Null);
            }
            let [arg] = args.as_slice() else {
                return None;
            };
            if tag == tags.bool_ {
                return scope.root_as_bool(*arg).ok().map(serde_json::Value::Bool);
            }
            if tag == tags.string {
                return scope
                    .root_as_string(*arg)
                    .ok()
                    .map(serde_json::Value::String);
            }
            if tag == tags.number {
                let number = scope.root_as_f64(*arg).ok()?;
                return serde_json::Number::from_f64(number)
                    .map(serde_json::Value::Number)
                    .or(Some(serde_json::Value::Null));
            }
            if tag == tags.array {
                let values = scope.root_as_list(*arg).ok()?;
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    out.push(to_serde_json(scope, value, tags)?);
                }
                return Some(serde_json::Value::Array(out));
            }
            if tag == tags.object {
                let values = scope.root_as_dict(*arg).ok()?;
                let mut out = serde_json::Map::with_capacity(values.len());
                for (name, value) in values {
                    out.insert(
                        name.as_ref().to_string(),
                        to_serde_json(scope, value, tags)?,
                    );
                }
                return Some(serde_json::Value::Object(out));
            }
            None
        }

        engine.export_native("prim_json_stringify", scheme, 1, move |scope, _, args| {
            let json = to_serde_json(scope, args[0], &tags);
            let Some(json) = json else {
                return scope.alloc_root_string("<non-std.json.Value>".into());
            };
            scope.alloc_root_string(json.to_string())
        })?;
    }

    // prim_json_parse : string -> Result a string
    //
    // This returns `Ok <std.json.Value>` when `a` is instantiated to the
    // qualified `std.json.Value` type. It's a primop, so we keep it minimal and
    // let `std.json.parse/from_string` wrap the string error into `DecodeError`.
    {
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(&string_ty, Type::result(a, &string_ty))
        );

        #[derive(Clone)]
        struct Tags {
            null: Symbol,
            bool_: Symbol,
            string: Symbol,
            number: Symbol,
            array: Symbol,
            object: Symbol,
        }

        let tags = Tags {
            null: Symbol::intern("Null"),
            bool_: Symbol::intern("Bool"),
            string: Symbol::intern("String"),
            number: Symbol::intern("Number"),
            array: Symbol::intern("Array"),
            object: Symbol::intern("Object"),
        };

        fn to_json_value(
            v: &serde_json::Value,
            tags: &Tags,
            scope: &mut RootScope<'_>,
        ) -> Result<RootedPtr, EngineError> {
            match v {
                serde_json::Value::Null => scope.alloc_root_adt(tags.null.clone(), vec![]),
                serde_json::Value::Bool(b) => {
                    let value = scope.alloc_root_bool(*b)?;
                    scope.alloc_root_adt(tags.bool_.clone(), vec![value])
                }
                serde_json::Value::String(s) => {
                    let value = scope.alloc_root_string(s.clone())?;
                    scope.alloc_root_adt(tags.string.clone(), vec![value])
                }
                serde_json::Value::Number(n) => {
                    let Some(f) = n.as_f64() else {
                        return Err(EngineError::Custom(
                            "expected JSON number representable as f64".into(),
                        ));
                    };
                    let value = scope.alloc_root_f64(f)?;
                    scope.alloc_root_adt(tags.number.clone(), vec![value])
                }
                serde_json::Value::Array(xs) => {
                    let mut out = Vec::with_capacity(xs.len());
                    for x in xs {
                        let value = to_json_value(x, tags, scope)?;
                        out.push(value);
                    }
                    let list = scope.alloc_root_list(out)?;
                    scope.alloc_root_adt(tags.array.clone(), vec![list])
                }
                serde_json::Value::Object(obj) => {
                    let mut out = BTreeMap::new();
                    for (k, v) in obj {
                        let value = to_json_value(v, tags, scope)?;
                        out.insert(Symbol::intern(k.as_str()), value);
                    }
                    let dict = scope.alloc_root_dict(out)?;
                    scope.alloc_root_adt(tags.object.clone(), vec![dict])
                }
            }
        }

        fn result_ok(
            scope: &mut RootScope<'_>,
            value: RootedPtr,
        ) -> Result<RootedPtr, EngineError> {
            scope.alloc_root_adt(Symbol::intern("Ok"), vec![value])
        }

        fn result_err(
            scope: &mut RootScope<'_>,
            message: String,
        ) -> Result<RootedPtr, EngineError> {
            let message = scope.alloc_root_string(message)?;
            scope.alloc_root_adt(Symbol::intern("Err"), vec![message])
        }

        engine.export_native("prim_json_parse", scheme, 1, move |scope, _, args| {
            let s = scope.root_as_string(args[0])?;
            let parsed: serde_json::Value = match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(e) => return result_err(scope, e.to_string()),
            };
            match to_json_value(&parsed, &tags, scope) {
                Ok(value) => result_ok(scope, value),
                Err(err) => result_err(scope, err.to_string()),
            }
        })?;
    }

    Ok(())
}

fn inject_list_builtins<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, b),
                Type::fun(Type::list(a), Type::list(b)),
            )
        );
        engine.export_native_scheduler("prim_map", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceMap(
                NativeSequenceMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::List,
                    children: Vec::new(),
                    output: Vec::new(),
                    remaining: 0,
                },
            )))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, b),
                Type::fun(Type::option(a), Type::option(b)),
            )
        );
        engine.export_native_scheduler("prim_map", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            match option_value(scope, args[1])? {
                Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryMap(
                    NativeUnaryMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        value,
                        shape: NativeUnaryShape::Option,
                    },
                ))),
                None => {
                    let root = option_from_root(scope, None)?;
                    Ok(SchedulerNativeResult::Ready(root))
                }
            }
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b, e] =>
            Type::fun(
                Type::fun(a, b),
                Type::fun(Type::result(a, e), Type::result(b, e)),
            )
        );
        engine.export_native_scheduler("prim_map", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            let (ok_ty, _err_ty) = result_types(&result_ty)?;
            match result_value(scope, args[1])? {
                Ok(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryMap(
                    NativeUnaryMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: ok_ty,
                        value,
                        shape: NativeUnaryShape::Result,
                    },
                ))),
                Err(err) => {
                    let root = result_from_root(scope, Err(err))?;
                    Ok(SchedulerNativeResult::Ready(root))
                }
            }
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(b, Type::fun(a, b)),
                Type::fun(b, Type::fun(Type::list(a), b)),
            )
        );
        engine.export_native_scheduler("prim_foldl", scheme, 3, |scope, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[2])?;
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(b, Type::fun(a, b)),
                Type::fun(b, Type::fun(Type::option(a), b)),
            )
        );
        engine.export_native_scheduler("prim_foldl", scheme, 3, |scope, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty)?;
            let values = ListItems::Pointers(option_value(scope, args[2])?.into_iter().collect());
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, Type::fun(b, b)),
                Type::fun(b, Type::fun(Type::list(a), b)),
            )
        );
        engine.export_native_scheduler("prim_foldr", scheme, 3, |scope, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[2])?;
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Right,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, Type::fun(b, b)),
                Type::fun(b, Type::fun(Type::option(a), b)),
            )
        );
        engine.export_native_scheduler("prim_foldr", scheme, 3, |scope, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty)?;
            let values = ListItems::Pointers(option_value(scope, args[2])?.into_iter().collect());
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Right,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(b, Type::fun(a, b)),
                Type::fun(b, Type::fun(Type::list(a), b)),
            )
        );
        engine.export_native_scheduler("prim_fold", scheme, 3, |scope, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[2])?;
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(b, Type::fun(a, b)),
                Type::fun(b, Type::fun(Type::option(a), b)),
            )
        );
        engine.export_native_scheduler("prim_fold", scheme, 3, |scope, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain(&call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty)?;
            let values = ListItems::Pointers(option_value(scope, args[2])?.into_iter().collect());
            Ok(SchedulerNativeResult::Task(NativeTask::Fold(NativeFold {
                func: args[0],
                func_type: func_ty,
                acc_type: acc_ty,
                elem_type: elem_ty,
                values,
                acc: args[1],
                order: NativeFoldOrder::Left,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::fun(a, Type::builtin(BuiltinTypeId::Bool)),
                Type::fun(Type::list(a), Type::list(a)),
            )
        );
        engine.export_native_scheduler("prim_filter", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceFilter(
                NativeSequenceFilter {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::List,
                    children: Vec::new(),
                    keep: Vec::new(),
                    remaining: 0,
                },
            )))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::fun(a, Type::builtin(BuiltinTypeId::Bool)),
                Type::fun(Type::option(a), Type::option(a)),
            )
        );
        engine.export_native_scheduler("prim_filter", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            match option_value(scope, args[1])? {
                Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFilter(
                    NativeUnaryFilter {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        value,
                        original: args[1],
                    },
                ))),
                None => {
                    let root = option_from_root(scope, None)?;
                    Ok(SchedulerNativeResult::Ready(root))
                }
            }
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, Type::option(b)),
                Type::fun(Type::list(a), Type::list(b)),
            )
        );
        engine.export_native_scheduler(
            "prim_filter_map",
            scheme,
            2,
            |scope, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let list_ty = arg_tys[1].clone();
                let elem_ty = list_elem_type(&list_ty)?;
                let values = scope.list_items(args[1])?;
                Ok(SchedulerNativeResult::Task(NativeTask::SequenceFilterMap(
                    NativeSequenceFilterMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        values,
                        shape: NativeSequenceShape::List,
                        children: Vec::new(),
                        output: Vec::new(),
                        remaining: 0,
                    },
                )))
            },
        )?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, Type::option(b)),
                Type::fun(Type::option(a), Type::option(b)),
            )
        );
        engine.export_native_scheduler(
            "prim_filter_map",
            scheme,
            2,
            |scope, call_type, args| {
                let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
                let func_ty = arg_tys[0].clone();
                let opt_ty = arg_tys[1].clone();
                let elem_ty = option_elem_type(&opt_ty)?;
                match option_value(scope, args[1])? {
                    Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFilterMap(
                        NativeUnaryFilterMap {
                            func: args[0],
                            func_type: func_ty,
                            elem_type: elem_ty,
                            value,
                        },
                    ))),
                    None => {
                        let root = option_from_root(scope, None)?;
                        Ok(SchedulerNativeResult::Ready(root))
                    }
                }
            },
        )?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, Type::list(b)),
                Type::fun(Type::list(a), Type::list(b)),
            )
        );
        engine.export_native_scheduler("prim_flat_map", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[1])?;
            Ok(SchedulerNativeResult::Task(NativeTask::SequenceFlatMap(
                NativeSequenceFlatMap {
                    func: args[0],
                    func_type: func_ty,
                    elem_type: elem_ty,
                    values,
                    shape: NativeSequenceShape::List,
                    children: Vec::new(),
                    output: Vec::new(),
                    remaining: 0,
                },
            )))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::fun(a, Type::option(b)),
                Type::fun(Type::option(a), Type::option(b)),
            )
        );
        engine.export_native_scheduler("prim_flat_map", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            match option_value(scope, args[1])? {
                Some(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFlatMap(
                    NativeUnaryFlatMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: elem_ty,
                        value,
                        shape: NativeUnaryShape::Option,
                    },
                ))),
                None => {
                    let root = option_from_root(scope, None)?;
                    Ok(SchedulerNativeResult::Ready(root))
                }
            }
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b, e] =>
            Type::fun(
                Type::fun(a, Type::result(b, e)),
                Type::fun(Type::result(a, e), Type::result(b, e)),
            )
        );
        engine.export_native_scheduler("prim_flat_map", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            let (ok_ty, _err_ty) = result_types(&result_ty)?;
            match result_value(scope, args[1])? {
                Ok(value) => Ok(SchedulerNativeResult::Task(NativeTask::UnaryFlatMap(
                    NativeUnaryFlatMap {
                        func: args[0],
                        func_type: func_ty,
                        elem_type: ok_ty,
                        value,
                        shape: NativeUnaryShape::Result,
                    },
                ))),
                Err(err) => {
                    let root = result_from_root(scope, Err(err))?;
                    Ok(SchedulerNativeResult::Ready(root))
                }
            }
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::fun(Type::list(a), Type::list(a)),
                Type::fun(Type::list(a), Type::list(a)),
            )
        );
        engine.export_native_scheduler("prim_or_else", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            if !scope.list_items(args[1])?.is_empty() {
                return Ok(SchedulerNativeResult::Ready(args[1]));
            }
            Ok(SchedulerNativeResult::Task(NativeTask::ApplyUnary(
                NativeApplyUnary {
                    func: args[0],
                    func_type: func_ty,
                    arg: args[1],
                    arg_type: list_ty,
                },
            )))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::fun(Type::option(a), Type::option(a)),
                Type::fun(Type::option(a), Type::option(a)),
            )
        );
        engine.export_native_scheduler("prim_or_else", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            if option_value(scope, args[1])?.is_some() {
                return Ok(SchedulerNativeResult::Ready(args[1]));
            }
            Ok(SchedulerNativeResult::Task(NativeTask::ApplyUnary(
                NativeApplyUnary {
                    func: args[0],
                    func_type: func_ty,
                    arg: args[1],
                    arg_type: opt_ty,
                },
            )))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, e] =>
            Type::fun(
                Type::fun(Type::result(a, e), Type::result(a, e)),
                Type::fun(Type::result(a, e), Type::result(a, e)),
            )
        );
        engine.export_native_scheduler("prim_or_else", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            if result_value(scope, args[1])?.is_ok() {
                return Ok(SchedulerNativeResult::Ready(args[1]));
            }
            Ok(SchedulerNativeResult::Task(NativeTask::ApplyUnary(
                NativeApplyUnary {
                    func: args[0],
                    func_type: func_ty,
                    arg: args[1],
                    arg_type: result_ty,
                },
            )))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::list(a), a)
        );
        engine.export_native_scheduler("sum", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[0])?;
            Ok(SchedulerNativeResult::Task(NativeTask::Sum(NativeSum {
                elem_type: elem_ty,
                values,
                acc: None,
                plus: None,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::option(a), a)
        );
        engine.export_native_scheduler("sum", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            let values = ListItems::Pointers(option_value(scope, args[0])?.into_iter().collect());
            Ok(SchedulerNativeResult::Task(NativeTask::Sum(NativeSum {
                elem_type: elem_ty,
                values,
                acc: None,
                plus: None,
                state: NativeFoldState::Enter,
                next_index: 0,
                step: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::list(a), a)
        );
        engine.export_native_scheduler("mean", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.list_items(args[0])?;
            if values.is_empty() {
                return Err(EngineError::EmptySequence);
            }
            Ok(SchedulerNativeResult::Task(NativeTask::Mean(NativeMean {
                len: values.len(),
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeMeanState::Enter,
                next_index: 0,
                step: None,
                len_value: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::option(a), a)
        );
        engine.export_native_scheduler("mean", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(&call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty)?;
            let values = match option_value(scope, args[0])? {
                Some(value) => ListItems::Pointers(vec![value]),
                None => return Err(EngineError::EmptySequence),
            };
            Ok(SchedulerNativeResult::Task(NativeTask::Mean(NativeMean {
                len: 1,
                elem_type: elem_ty,
                values,
                acc: None,
                state: NativeMeanState::Enter,
                next_index: 0,
                step: None,
                len_value: None,
            })))
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::list(a), Type::builtin(BuiltinTypeId::I32))
        );
        engine.export_native("prim_list_length", scheme, 1, |scope, _, args| {
            let values = scope.root_as_list(args[0])?;
            scope.alloc_root_i32(values.len() as i32)
        })?;
    }

    {
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let scheme = scheme!(Type::fun(&string_ty, Type::builtin(BuiltinTypeId::I32),));
        engine.export_native("prim_string_length", scheme, 1, |scope, _, args| {
            let value = scope.root_as_string(args[0])?;
            let length = i32::try_from(value.chars().count())
                .map_err(|_| EngineError::Internal("string length overflow".into()))?;
            scope.alloc_root_i32(length)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(Type::list(a), Type::list(a)),
            )
        );
        engine.export_native("first", scheme, 2, |scope, _, args| {
            let n = scope.root_as_i32(args[0])?;
            let values = scope.root_as_list(args[1])?;
            let end = checked_endpoint(Symbol::intern("first"), n, values.len())?;
            list_range_from_items(scope, values, 0, end)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(Type::list(a), Type::list(a)),
            )
        );
        engine.export_native("last", scheme, 2, |scope, _, args| {
            let n = scope.root_as_i32(args[0])?;
            let values = scope.root_as_list(args[1])?;
            let len = values.len();
            let n = checked_endpoint(Symbol::intern("last"), n, len)?;
            let start = len - n;
            list_range_from_items(scope, values, start, len)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(
                    Type::builtin(BuiltinTypeId::I32),
                    Type::fun(Type::list(a), Type::list(a)),
                ),
            )
        );
        engine.export_native("slice", scheme, 3, |scope, _, args| {
            let n = scope.root_as_i32(args[0])?;
            let m = scope.root_as_i32(args[1])?;
            let values = scope.root_as_list(args[2])?;
            let start = checked_endpoint(Symbol::intern("slice"), n, values.len())?;
            let end = checked_endpoint(Symbol::intern("slice"), m, values.len())?;
            if end < start {
                return Err(EngineError::Custom(format!(
                    "invalid slice range: end {m} is before start {n}"
                )));
            }
            list_range_from_items(scope, values, start, end)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(Type::list(a), Type::list(a)),
            )
        );
        engine.export_native("prim_take", scheme, 2, |scope, _, args| {
            let n = scope.root_as_i32(args[0])?;
            let n = as_nonneg_usize(n);
            let values = scope.root_as_list(args[1])?;
            let end = values.len().min(n);
            list_range_from_items(scope, values, 0, end)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(Type::list(a), Type::list(a)),
            )
        );
        engine.export_native("prim_skip", scheme, 2, |scope, _, args| {
            let n = scope.root_as_i32(args[0])?;
            let n = as_nonneg_usize(n);
            let values = scope.root_as_list(args[1])?;
            let len = values.len();
            let start = len.min(n);
            list_range_from_items(scope, values, start, len)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(Type::list(a), a),
            )
        );
        engine.export_native("prim_get", scheme, 2, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let list_ty = arg_tys[1].clone();
            let _elem_ty = list_elem_type(&list_ty)?;
            let idx = scope.root_as_i32(args[0])?;
            let values = scope.root_as_list(args[1])?;
            let idx = checked_index(Symbol::intern("prim_get"), idx, values.len())?;
            Ok(values[idx])
        })?;
    }

    for size in 2..=32 {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(Type::tuple(vec![a; size]), a),
            )
        );
        engine.export_native("prim_get", scheme, 2, move |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 2)?;
            let tuple_ty = arg_tys[1].clone();
            let _elem_ty = tuple_elem_type(&tuple_ty)?;
            let idx = scope.root_as_i32(args[0])?;
            let idx_usize = checked_index(Symbol::intern("prim_get"), idx, size)?;
            let xs = scope.root_as_tuple(args[1])?;
            if xs.len() != size {
                return Err(EngineError::NativeType {
                    expected: format!("tuple{}", size),
                    got: format!("tuple{}", xs.len()),
                });
            }
            Ok(xs[idx_usize])
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::list(a),
                Type::fun(Type::list(b), Type::list(Type::tuple(vec![a, b]))),
            )
        );
        engine.export_native("prim_zip", scheme, 2, |scope, _, args| {
            let xs = scope.root_as_list(args[0])?;
            let ys = scope.root_as_list(args[1])?;
            let zipped = zip_tuple2_roots(scope, xs, ys)?;
            scope.alloc_root_list(zipped)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a, b] =>
            Type::fun(
                Type::list(Type::tuple(vec![a, b])),
                Type::tuple(vec![Type::list(a), Type::list(b)]),
            )
        );
        engine.export_native("prim_unzip", scheme, 1, |scope, _, args| {
            let pairs = scope.root_as_list(args[0])?;
            let (left, right) = unzip_tuple2_roots(scope, pairs)?;
            let left = scope.alloc_root_list(left)?;
            let right = scope.alloc_root_list(right)?;
            scope.alloc_root_tuple(vec![left, right])
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::list(a), a)
        );
        engine.export_native("min", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.root_as_list(args[0])?;
            extremum_root_by_type(scope, &elem_ty, values, std::cmp::Ordering::Less)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::option(a), a)
        );
        engine.export_native("min", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty)?;
            match option_value(scope, args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::list(a), a)
        );
        engine.export_native("max", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty)?;
            let values = scope.root_as_list(args[0])?;
            extremum_root_by_type(scope, &elem_ty, values, std::cmp::Ordering::Greater)
        })?;
    }

    {
        let scheme = scheme!(&mut engine.type_system.supply; forall [a] =>
            Type::fun(Type::option(a), a)
        );
        engine.export_native("max", scheme, 1, |scope, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain(call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty)?;
            match option_value(scope, args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    Ok(())
}

fn inject_option_result_builtins<State: Clone + Send + Sync + 'static>(
    engine: &mut Builder<State>,
) -> Result<(), EngineError> {
    let unwrap = Symbol::intern("unwrap");
    let unwrap_schemes = engine
        .type_system
        .env
        .lookup(&unwrap)
        .ok_or_else(|| EngineError::UnknownVar(unwrap.clone()))?
        .to_vec();
    for scheme in unwrap_schemes {
        let typ = scheme.typ.clone();
        match typ.as_ref() {
            TypeKind::Fun(arg_ty, _)
                if matches!(
                    arg_ty.as_ref(),
                    TypeKind::App(head, _)
                        if matches!(
                            head.as_ref(),
                            TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Option)
                        )
                ) =>
            {
                engine.export_native("unwrap", scheme, 1, |scope, _, args| match option_value(
                    scope, args[0],
                )? {
                    Some(value) => Ok(value),
                    None => Err(EngineError::Custom("called unwrap on None".into())),
                })?;
            }
            TypeKind::Fun(arg_ty, _)
                if matches!(
                    arg_ty.as_ref(),
                    TypeKind::App(head, _)
                        if matches!(
                            head.as_ref(),
                            TypeKind::App(head2, _)
                                if matches!(
                                    head2.as_ref(),
                                    TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::Result)
                                )
                        )
                ) =>
            {
                engine.export_native("unwrap", scheme, 1, |scope, _, args| match result_value(
                    scope, args[0],
                )? {
                    Ok(value) => Ok(value),
                    Err(_) => Err(EngineError::Custom("called unwrap on Err".into())),
                })?;
            }
            _ => {}
        }
    }

    let is_some = Symbol::intern("is_some");
    let is_some_scheme = engine.lookup_scheme(&is_some)?;
    engine.export_native("is_some", is_some_scheme, 1, |scope, _, args| {
        let value = option_value(scope, args[0])?.is_some();
        scope.alloc_root_bool(value)
    })?;
    let is_none = Symbol::intern("is_none");
    let is_none_scheme = engine.lookup_scheme(&is_none)?;
    engine.export_native("is_none", is_none_scheme, 1, |scope, _, args| {
        let value = option_value(scope, args[0])?.is_none();
        scope.alloc_root_bool(value)
    })?;

    let is_ok = Symbol::intern("is_ok");
    let is_ok_scheme = engine.lookup_scheme(&is_ok)?;
    engine.export_native("is_ok", is_ok_scheme, 1, |scope, _, args| {
        let value = result_value(scope, args[0])?.is_ok();
        scope.alloc_root_bool(value)
    })?;
    let is_err = Symbol::intern("is_err");
    let is_err_scheme = engine.lookup_scheme(&is_err)?;
    engine.export_native("is_err", is_err_scheme, 1, |scope, _, args| {
        let value = result_value(scope, args[0])?.is_err();
        scope.alloc_root_bool(value)
    })?;
    Ok(())
}

fn binary_arg_types(typ: &Type) -> Result<(Type, Type), EngineError> {
    let (lhs, rest) = split_fun(typ).ok_or_else(|| EngineError::NativeType {
        expected: "binary function".into(),
        got: typ.to_string(),
    })?;
    let (rhs, _res) = split_fun(&rest).ok_or_else(|| EngineError::NativeType {
        expected: "binary function".into(),
        got: typ.to_string(),
    })?;
    Ok((lhs, rhs))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    use rex_ast::{Decl, Expr, Symbol};
    use rex_typesystem::{
        types::{Predicate, Scheme, Type, Types},
        typesystem::{TypeVarSupply, entails, instantiate},
        unification::unify,
    };

    use super::*;

    fn is_primitive_name(name: &Symbol) -> bool {
        name.as_ref().starts_with("prim_")
    }

    // TODO: Consider adding a generic visitor function to `Expr` so callers do
    // not need to hand-roll recursive AST walks like this.
    fn collect_primitive_expr_refs(expr: &Arc<Expr>, out: &mut BTreeSet<Symbol>) {
        match expr.as_ref() {
            Expr::Var(var) => {
                if is_primitive_name(&var.name) {
                    out.insert(var.name.clone());
                }
            }
            Expr::Tuple(_, elems) | Expr::List(_, elems) => {
                for elem in elems {
                    collect_primitive_expr_refs(elem, out);
                }
            }
            Expr::Dict(_, fields) => {
                for value in fields.values() {
                    collect_primitive_expr_refs(value, out);
                }
            }
            Expr::RecordUpdate(_, base, updates) => {
                collect_primitive_expr_refs(base, out);
                for value in updates.values() {
                    collect_primitive_expr_refs(value, out);
                }
            }
            Expr::App(_, f, x) => {
                collect_primitive_expr_refs(f, out);
                collect_primitive_expr_refs(x, out);
            }
            Expr::Project(_, base, _) | Expr::Ann(_, base, _) => {
                collect_primitive_expr_refs(base, out);
            }
            Expr::Lam(_, scope, _, _, _, body) => {
                for captured in scope.values() {
                    collect_primitive_expr_refs(captured, out);
                }
                collect_primitive_expr_refs(body, out);
            }
            Expr::Let(_, _, _, _, def, body) => {
                collect_primitive_expr_refs(def, out);
                collect_primitive_expr_refs(body, out);
            }
            Expr::LetRec(_, bindings, body) => {
                for (_, _, _, def) in bindings {
                    collect_primitive_expr_refs(def, out);
                }
                collect_primitive_expr_refs(body, out);
            }
            Expr::Ite(_, cond, then_expr, else_expr) => {
                collect_primitive_expr_refs(cond, out);
                collect_primitive_expr_refs(then_expr, out);
                collect_primitive_expr_refs(else_expr, out);
            }
            Expr::Match(_, scrutinee, arms) => {
                collect_primitive_expr_refs(scrutinee, out);
                for (_, arm) in arms {
                    collect_primitive_expr_refs(arm, out);
                }
            }
            Expr::Bool(..)
            | Expr::Uint(..)
            | Expr::Int(..)
            | Expr::Float(..)
            | Expr::String(..)
            | Expr::Uuid(..)
            | Expr::DateTime(..)
            | Expr::Hole(..) => {}
        }
    }

    fn primitive_refs_in_prelude_source() -> BTreeSet<Symbol> {
        let mut refs = BTreeSet::new();
        let program = prelude_typeclasses_program().unwrap();
        for decl in &program.decls {
            match decl {
                Decl::Fn(fd) => collect_primitive_expr_refs(&fd.body, &mut refs),
                Decl::Instance(inst) => {
                    for method in &inst.methods {
                        collect_primitive_expr_refs(&method.body, &mut refs);
                    }
                }
                Decl::Type(..) | Decl::Class(..) | Decl::DeclareFn(..) | Decl::Import(..) => {}
            }
        }
        refs
    }

    fn primitive_schemes_in_standard_type_system() -> BTreeMap<Symbol, Vec<Scheme>> {
        let ts = standard_type_system().unwrap();
        ts.env
            .values
            .iter()
            .filter(|(name, _)| is_primitive_name(name))
            .map(|(name, schemes)| (name.clone(), schemes.clone()))
            .collect()
    }

    fn primitive_schemes_in_runtime() -> BTreeMap<Symbol, Vec<Scheme>> {
        let builder = Builder::with_prelude(()).unwrap();
        builder
            .runtime
            .natives
            .schemes()
            .filter(|(name, _)| is_primitive_name(name))
            .map(|(name, schemes)| (name.clone(), schemes))
            .collect()
    }

    fn scheme_accepts(
        classes: &rex_typesystem::types::ClassEnv,
        scheme: &Scheme,
        typ: &Type,
    ) -> bool {
        let mut supply = TypeVarSupply::new();
        let (preds, scheme_ty) = instantiate(scheme, &mut supply);
        let Ok(subst) = unify(&scheme_ty, typ) else {
            return false;
        };
        let preds: Vec<Predicate> = preds.apply(&subst);
        for pred in preds {
            if pred.typ.ftv().is_empty() && !entails(classes, &[], &pred).unwrap() {
                return false;
            }
        }
        true
    }

    fn has_compatible_scheme(
        classes: &rex_typesystem::types::ClassEnv,
        candidates: &[Scheme],
        scheme: &Scheme,
    ) -> bool {
        candidates
            .iter()
            .any(|candidate| scheme_accepts(classes, candidate, &scheme.typ))
    }

    #[test]
    fn prelude_primitive_names_are_consistent_across_source_types_and_runtime() {
        let source_refs = primitive_refs_in_prelude_source();
        let typed = primitive_schemes_in_standard_type_system();
        let runtime = primitive_schemes_in_runtime();

        assert!(
            !source_refs.is_empty(),
            "expected the prelude source to reference rust-backed primitives"
        );

        let typed_names = typed.keys().cloned().collect::<BTreeSet<_>>();
        let runtime_names = runtime.keys().cloned().collect::<BTreeSet<_>>();

        let missing_from_types = source_refs
            .difference(&typed_names)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing_from_types.is_empty(),
            "prelude source references primitives with no standard type schemes: {missing_from_types:?}"
        );

        let missing_from_runtime = source_refs
            .difference(&runtime_names)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing_from_runtime.is_empty(),
            "prelude source references primitives with no runtime implementation: {missing_from_runtime:?}"
        );

        let typed_only = typed_names
            .difference(&runtime_names)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            typed_only.is_empty(),
            "standard type system contains primitives with no runtime implementation: {typed_only:?}"
        );

        let runtime_only = runtime_names
            .difference(&typed_names)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            runtime_only.is_empty(),
            "runtime registers primitives with no standard type scheme: {runtime_only:?}"
        );
    }

    #[test]
    fn prelude_primitive_schemes_are_compatible_between_types_and_runtime() {
        let ts = standard_type_system().unwrap();
        let typed = primitive_schemes_in_standard_type_system();
        let runtime = primitive_schemes_in_runtime();

        for (name, runtime_schemes) in &runtime {
            let typed_schemes = typed
                .get(name)
                .unwrap_or_else(|| panic!("missing standard type schemes for primitive `{name}`"));
            for runtime_scheme in runtime_schemes {
                assert!(
                    has_compatible_scheme(&ts.classes, typed_schemes, runtime_scheme),
                    "runtime primitive `{name}` has scheme `{runtime_scheme:?}` not accepted by the standard type system"
                );
            }
        }

        for (name, typed_schemes) in &typed {
            let runtime_schemes = runtime
                .get(name)
                .unwrap_or_else(|| panic!("missing runtime schemes for primitive `{name}`"));
            for typed_scheme in typed_schemes {
                assert!(
                    has_compatible_scheme(&ts.classes, runtime_schemes, typed_scheme),
                    "standard primitive `{name}` has scheme `{typed_scheme:?}` not covered by runtime registration"
                );
            }
        }
    }
}
