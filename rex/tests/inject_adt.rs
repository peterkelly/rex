use std::collections::BTreeMap;

use rex::{
    AdtDecl, BuiltinTypeId, Engine, EngineError, FromPointer, GasMeter, Heap, IntoPointer, Parser,
    Pointer, Rex, RexAdt, RexType, Token, Type, TypeVarSupply, assert_pointer_eq, sym,
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

    fn collect_rex_family(out: &mut Vec<AdtDecl>) -> Result<(), EngineError> {
        out.push(<Self as RexAdt>::rex_adt_decl()?);
        Ok(())
    }
}

impl IntoPointer for ManualRecord {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        let mut fields = BTreeMap::new();
        fields.insert(sym("enabled"), self.enabled.into_pointer(heap)?);
        fields.insert(sym("count"), self.count.into_pointer(heap)?);
        let dict = heap.alloc_dict(fields)?;
        heap.alloc_adt(sym("ManualRecord"), vec![dict])
    }
}

impl FromPointer for ManualRecord {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (tag, args) = heap.pointer_as_adt(pointer)?;
        if tag.as_ref() != "ManualRecord" || args.len() != 1 {
            return Err(EngineError::NativeType {
                expected: "ManualRecord".into(),
                got: heap.type_name(pointer)?.into(),
            });
        }

        let fields = heap.pointer_as_dict(&args[0])?;
        let enabled = fields
            .get(&sym("enabled"))
            .ok_or_else(|| EngineError::NativeType {
                expected: "field `enabled`".into(),
                got: "dict".into(),
            })
            .and_then(|p| bool::from_pointer(heap, p))?;
        let count = fields
            .get(&sym("count"))
            .ok_or_else(|| EngineError::NativeType {
                expected: "field `count`".into(),
                got: "dict".into(),
            })
            .and_then(|p| i32::from_pointer(heap, p))?;

        Ok(Self { enabled, count })
    }
}

impl RexType for ManualEnum {
    fn rex_type() -> Type {
        Type::con("ManualEnum", 0)
    }

    fn collect_rex_family(out: &mut Vec<AdtDecl>) -> Result<(), EngineError> {
        out.push(<Self as RexAdt>::rex_adt_decl()?);
        Ok(())
    }
}

impl IntoPointer for ManualEnum {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        match self {
            Self::Flag(value) => {
                let value = value.into_pointer(heap)?;
                heap.alloc_adt(sym("Flag"), vec![value])
            }
            Self::Count(value) => {
                let value = value.into_pointer(heap)?;
                heap.alloc_adt(sym("Count"), vec![value])
            }
        }
    }
}

impl FromPointer for ManualEnum {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (tag, args) = heap.pointer_as_adt(pointer)?;
        if tag.as_ref() == "Flag" && args.len() == 1 {
            return Ok(Self::Flag(bool::from_pointer(heap, &args[0])?));
        }
        if tag.as_ref() == "Count" && args.len() == 1 {
            return Ok(Self::Count(i32::from_pointer(heap, &args[0])?));
        }

        Err(EngineError::NativeType {
            expected: "ManualEnum".into(),
            got: heap.type_name(pointer)?.into(),
        })
    }
}

impl RexAdt for ManualRecord {
    fn rex_adt_decl() -> Result<AdtDecl, EngineError> {
        let mut supply = TypeVarSupply::new();
        let mut adt = AdtDecl::new(&sym("ManualRecord"), &[], &mut supply);
        let record = Type::record(vec![
            (sym("enabled"), bool::rex_type()),
            (sym("count"), i32::rex_type()),
        ]);
        adt.add_variant(sym("ManualRecord"), vec![record]);
        Ok(adt)
    }
}

impl RexAdt for ManualEnum {
    fn rex_adt_decl() -> Result<AdtDecl, EngineError> {
        let mut supply = TypeVarSupply::new();
        let mut adt = AdtDecl::new(&sym("ManualEnum"), &[], &mut supply);
        adt.add_variant(sym("Flag"), vec![bool::rex_type()]);
        adt.add_variant(sym("Count"), vec![i32::rex_type()]);
        Ok(adt)
    }
}

#[tokio::test]
async fn manual_struct_adt_can_be_registered_and_roundtripped() {
    let mut engine = Engine::with_prelude(()).unwrap();
    ManualRecord::inject_rex(&mut engine).unwrap();

    let tokens = Token::tokenize("ManualRecord { enabled = true, count = 41 }").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, ManualRecord::rex_type());
    let decoded = ManualRecord::from_pointer(&engine.heap, &ptr).unwrap();
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

    let tokens = Token::tokenize("DerivedRecord { enabled = true, count = 41 }").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, DerivedRecord::rex_type());
    let decoded = DerivedRecord::from_pointer(&engine.heap, &ptr).unwrap();
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
    ManualEnum::inject_rex(&mut engine).unwrap();

    let tokens = Token::tokenize(
        r#"
        match (Count 9)
            when Flag b -> if b then 1 else 0
            when Count n -> n + 1
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, ptr, engine.heap.alloc_i32(10).unwrap());
}

#[tokio::test]
async fn derived_enum_adt_can_be_registered_and_pattern_matched() {
    let mut engine = Engine::with_prelude(()).unwrap();
    DerivedEnum::inject_rex(&mut engine).unwrap();

    let tokens = Token::tokenize(
        r#"
        match (Count 9)
            when Flag b -> if b then 1 else 0
            when Count n -> n + 1
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, ptr, engine.heap.alloc_i32(10).unwrap());
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
    let t = adt.param_type(&sym("T")).unwrap();
    adt.add_variant(sym("Wrap"), vec![t]);
    let mut library = rex::Library::global();
    library.add_adt_decl(adt).unwrap();
    engine.inject_library(library).unwrap();

    let tokens = Token::tokenize(
        r#"
        match (Wrap 9)
            when Wrap x -> x + 1
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, ptr, engine.heap.alloc_i32(10).unwrap());
}

#[tokio::test]
async fn adt_decl_from_type_with_params_can_register_generic_adt_for_derived_types() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut adt = engine
        .adt_decl_from_type_with_params(&DerivedBox::<i32>::rex_type(), &["T"])
        .unwrap();
    let t = adt.param_type(&sym("T")).unwrap();
    adt.add_variant(sym("Boxed"), vec![t]);
    let mut library = rex::Library::global();
    library.add_adt_decl(adt).unwrap();
    engine.inject_library(library).unwrap();

    let tokens = Token::tokenize(
        r#"
        match (Boxed 9)
            when Boxed x -> x + 1
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, ptr, engine.heap.alloc_i32(10).unwrap());
}
