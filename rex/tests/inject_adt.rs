mod common;

use std::collections::BTreeMap;

use rex::{
    Rex,
    ast::Symbol,
    engine::{Engine, EngineError, FromRex, Handle, Heap, IntoRex, Module, Value},
    parser::parse as parse_rex,
    typesystem::{AdtDecl, BuiltinTypeId, RexAdt, RexType, Type, TypeError, TypeVarSupply},
};

#[derive(Debug, Clone, PartialEq)]
struct ManualRecord {
    enabled: bool,
    count: i32,
}

#[derive(Debug, Clone, PartialEq)]
enum ManualEnum {
    Flag(bool),
    Count(i32),
}

#[derive(Rex, Debug, Clone, PartialEq)]
struct DerivedRecord {
    enabled: bool,
    count: i32,
}

#[derive(Rex, Debug, Clone, PartialEq)]
enum DerivedEnum {
    Flag(bool),
    Count(i32),
}

#[derive(Rex, Debug, Clone, PartialEq)]
enum DerivedBox<T> {
    Boxed(T),
}

impl RexType for ManualRecord {
    fn rex_type() -> Type {
        Type::con("ManualRecord", 0)
    }

    fn collect_rex_family(out: &mut Vec<AdtDecl>) -> Result<(), TypeError> {
        out.push(<Self as RexAdt>::rex_adt_decl()?);
        Ok(())
    }
}

impl IntoRex for ManualRecord {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        let mut fields = BTreeMap::new();
        fields.insert(Symbol::intern("enabled"), self.enabled.into_rex(heap)?);
        fields.insert(Symbol::intern("count"), self.count.into_rex(heap)?);
        let dict = heap.alloc_dict(fields)?;
        heap.alloc_adt(Symbol::intern("ManualRecord"), vec![dict])
    }
}

impl FromRex for ManualRecord {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let Value::Adt(tag, args) = handle.value()? else {
            return Err(EngineError::NativeType {
                expected: "ManualRecord".into(),
                got: handle.type_name()?.into(),
            });
        };
        if tag.as_ref() != "ManualRecord" || args.len() != 1 {
            return Err(EngineError::NativeType {
                expected: "ManualRecord".into(),
                got: handle.type_name()?.into(),
            });
        }

        let Value::Dict(fields) = args[0].value()? else {
            return Err(EngineError::NativeType {
                expected: "dict".into(),
                got: args[0].type_name()?.into(),
            });
        };
        let enabled = fields
            .get(&Symbol::intern("enabled"))
            .ok_or_else(|| EngineError::NativeType {
                expected: "field `enabled`".into(),
                got: "dict".into(),
            })
            .and_then(bool::from_rex)?;
        let count = fields
            .get(&Symbol::intern("count"))
            .ok_or_else(|| EngineError::NativeType {
                expected: "field `count`".into(),
                got: "dict".into(),
            })
            .and_then(i32::from_rex)?;

        Ok(Self { enabled, count })
    }
}

impl RexType for ManualEnum {
    fn rex_type() -> Type {
        Type::con("ManualEnum", 0)
    }

    fn collect_rex_family(out: &mut Vec<AdtDecl>) -> Result<(), TypeError> {
        out.push(<Self as RexAdt>::rex_adt_decl()?);
        Ok(())
    }
}

impl IntoRex for ManualEnum {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        match self {
            Self::Flag(value) => {
                let value = value.into_rex(heap)?;
                heap.alloc_adt(Symbol::intern("Flag"), vec![value])
            }
            Self::Count(value) => {
                let value = value.into_rex(heap)?;
                heap.alloc_adt(Symbol::intern("Count"), vec![value])
            }
        }
    }
}

impl FromRex for ManualEnum {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let Value::Adt(tag, args) = handle.value()? else {
            return Err(EngineError::NativeType {
                expected: "ManualEnum".into(),
                got: handle.type_name()?.into(),
            });
        };
        if tag.as_ref() == "Flag" && args.len() == 1 {
            return Ok(Self::Flag(bool::from_rex(&args[0])?));
        }
        if tag.as_ref() == "Count" && args.len() == 1 {
            return Ok(Self::Count(i32::from_rex(&args[0])?));
        }

        Err(EngineError::NativeType {
            expected: "ManualEnum".into(),
            got: handle.type_name()?.into(),
        })
    }
}

impl RexAdt for ManualRecord {
    fn rex_adt_decl() -> Result<AdtDecl, TypeError> {
        let mut supply = TypeVarSupply::new();
        let mut adt = AdtDecl::new(&Symbol::intern("ManualRecord"), &[], &mut supply);
        let record = Type::record(vec![
            (Symbol::intern("enabled"), bool::rex_type()),
            (Symbol::intern("count"), i32::rex_type()),
        ]);
        adt.add_variant(Symbol::intern("ManualRecord"), vec![record]);
        Ok(adt)
    }
}

impl RexAdt for ManualEnum {
    fn rex_adt_decl() -> Result<AdtDecl, TypeError> {
        let mut supply = TypeVarSupply::new();
        let mut adt = AdtDecl::new(&Symbol::intern("ManualEnum"), &[], &mut supply);
        adt.add_variant(Symbol::intern("Flag"), vec![bool::rex_type()]);
        adt.add_variant(Symbol::intern("Count"), vec![i32::rex_type()]);
        Ok(adt)
    }
}

#[tokio::test]
async fn manual_struct_adt_can_be_registered_and_roundtripped() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_rex_adt::<ManualRecord>().unwrap();

    let program = parse_rex("ManualRecord { enabled = true, count = 41 }").unwrap();
    let (handle, ty) = common::run_program_body(engine, &program).await.unwrap();
    assert_eq!(ty, ManualRecord::rex_type());
    let decoded = ManualRecord::from_rex(&handle).unwrap();
    assert_eq!(
        decoded,
        ManualRecord {
            enabled: true,
            count: 41
        }
    );
}

#[tokio::test]
async fn derived_struct_adt_can_be_registered_and_roundtripped() {
    let mut engine = Engine::with_prelude(()).unwrap();
    DerivedRecord::inject_rex(&mut engine).unwrap();

    let program = parse_rex("DerivedRecord { enabled = true, count = 41 }").unwrap();
    let (handle, ty) = common::run_program_body(engine, &program).await.unwrap();
    assert_eq!(ty, DerivedRecord::rex_type());
    let decoded = DerivedRecord::from_rex(&handle).unwrap();
    assert_eq!(
        decoded,
        DerivedRecord {
            enabled: true,
            count: 41
        }
    );
}

#[tokio::test]
async fn manual_enum_adt_can_be_registered_and_pattern_matched() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_rex_adt::<ManualEnum>().unwrap();

    let program = parse_rex(
        r#"
        match (Count 9) with {
            case Flag b -> if b then 1 else 0;
            case Count n -> n + 1;
        }
        "#,
    )
    .unwrap();
    let (handle, ty) = common::run_program_body(engine, &program).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(handle.as_i32().unwrap(), 10);
}

#[tokio::test]
async fn derived_enum_adt_can_be_registered_and_pattern_matched() {
    let mut engine = Engine::with_prelude(()).unwrap();
    DerivedEnum::inject_rex(&mut engine).unwrap();

    let program = parse_rex(
        r#"
        match (Count 9) with {
            case Flag b -> if b then 1 else 0;
            case Count n -> n + 1;
        }
        "#,
    )
    .unwrap();
    let (handle, ty) = common::run_program_body(engine, &program).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(handle.as_i32().unwrap(), 10);
}

#[test]
fn adt_decl_from_type_rejects_non_constructor_heads() {
    let mut engine = Engine::new(());
    let err = engine
        .adt_decl_from_type(&Type::tuple(vec![Type::builtin(BuiltinTypeId::I32)]))
        .unwrap_err();
    let EngineError::Custom(message) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(message.contains("non-constructor type"));
}

#[test]
fn adt_decl_from_type_rejects_non_constructor_heads_for_derived_types() {
    let mut engine = Engine::new(());
    let err = engine
        .adt_decl_from_type(&Type::tuple(vec![DerivedRecord::rex_type()]))
        .unwrap_err();
    let EngineError::Custom(message) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(message.contains("non-constructor type"));
}

#[test]
fn adt_decl_from_type_rejects_applied_non_variable_args() {
    let mut engine = Engine::new(());
    let typ = Type::app(Type::con("Boxed", 1), Type::builtin(BuiltinTypeId::I32));
    let err = engine.adt_decl_from_type(&typ).unwrap_err();
    let EngineError::Custom(message) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(message.contains("expected type variables"));
}

#[test]
fn adt_decl_from_type_rejects_applied_non_variable_args_for_derived_types() {
    let mut engine = Engine::new(());
    let err = engine
        .adt_decl_from_type(&DerivedBox::<i32>::rex_type())
        .unwrap_err();
    let EngineError::Custom(message) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(message.contains("expected type variables"));
}

#[test]
fn adt_decl_from_type_with_params_validates_arity() {
    let mut engine = Engine::new(());
    let err = engine
        .adt_decl_from_type_with_params(&Type::builtin(BuiltinTypeId::Result), &["T"])
        .unwrap_err();
    let EngineError::Custom(message) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(message.contains("expects 2 parameters"));
}

#[test]
fn adt_decl_from_type_with_params_validates_arity_for_derived_types() {
    let mut engine = Engine::new(());
    let err = engine
        .adt_decl_from_type_with_params(&DerivedBox::<i32>::rex_type(), &[])
        .unwrap_err();
    let EngineError::Custom(message) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(message.contains("expects 1 parameters"));
}

#[tokio::test]
async fn adt_decl_from_type_with_params_can_register_generic_adt() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut adt = engine
        .adt_decl_from_type_with_params(&Type::con("Wrap", 1), &["T"])
        .unwrap();
    let t = adt.param_type(&Symbol::intern("T")).unwrap();
    adt.add_variant(Symbol::intern("Wrap"), vec![t]);
    let mut module = Module::global();
    module.add_adt_decl(adt).unwrap();
    engine.inject_module(module).unwrap();

    let program = parse_rex(
        r#"
        match (Wrap 9) with {
            case Wrap x -> x + 1;
        }
        "#,
    )
    .unwrap();
    let (handle, ty) = common::run_program_body(engine, &program).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(handle.as_i32().unwrap(), 10);
}

#[tokio::test]
async fn adt_decl_from_type_with_params_can_register_generic_adt_for_derived_types() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut adt = engine
        .adt_decl_from_type_with_params(&DerivedBox::<i32>::rex_type(), &["T"])
        .unwrap();
    let t = adt.param_type(&Symbol::intern("T")).unwrap();
    adt.add_variant(Symbol::intern("Boxed"), vec![t]);
    let mut module = Module::global();
    module.add_adt_decl(adt).unwrap();
    engine.inject_module(module).unwrap();

    let program = parse_rex(
        r#"
        match (Boxed 9) with {
            case Boxed x -> x + 1;
        }
        "#,
    )
    .unwrap();
    let (handle, ty) = common::run_program_body(engine, &program).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(handle.as_i32().unwrap(), 10);
}
