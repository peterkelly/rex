//! Type-directed conversion between JSON and owned Rex values.

use crate::engine::{EngineError, MainSignature, Value as RexValue};
use blake3::Hash;
use rex_ast::Symbol;
use rex_typesystem::{
    types::{AdtDecl, BuiltinTypeId, Type, TypeKind},
    typesystem::TypeSystem,
};
use serde_json::{Map, Number, Value as JsonValue};
use std::collections::BTreeMap;

fn local_name(name: &Symbol) -> &str {
    name.as_ref().rsplit('.').next().unwrap_or(name.as_ref())
}

fn constructor_matches(actual: &Symbol, expected: &Symbol) -> bool {
    actual == expected
        || (!actual.as_ref().contains('.') && actual.as_ref() == local_name(expected))
}

fn local_name_matches(name: &Symbol, expected: &str) -> bool {
    local_name(name) == expected
}

/// Convert JSON into Rex's owned host representation using a concrete Rex type.
pub fn json_to_rex(
    json: &JsonValue,
    want: &Type,
    ts: &TypeSystem,
) -> Result<RexValue, EngineError> {
    match want.as_ref() {
        TypeKind::Var(tv) => Err(error(format!(
            "cannot decode JSON into unresolved type variable t{}",
            tv.id
        ))),
        TypeKind::Con(con) => json_to_value_for_con(json, &con.name(), &[], ts),
        TypeKind::App(_, _) => {
            let (head, args) = decompose_type_app(want);
            let TypeKind::Con(con) = head.as_ref() else {
                return Err(error(format!("unsupported applied type {want}")));
            };
            json_to_value_for_con(json, &con.name(), &args, ts)
        }
        TypeKind::Fun(_, _) => Err(error("cannot decode JSON into function type".into())),
        TypeKind::Tuple(item_types) => {
            let JsonValue::Array(items) = json else {
                return Err(type_mismatch_json(json, want));
            };
            if items.len() != item_types.len() {
                return Err(type_mismatch_json(json, want));
            }
            items
                .iter()
                .zip(item_types)
                .map(|(item, typ)| json_to_rex(item, typ, ts))
                .collect::<Result<Vec<_>, _>>()
                .map(RexValue::Tuple)
        }
        TypeKind::Record(field_types) => {
            let JsonValue::Object(fields) = json else {
                return Err(type_mismatch_json(json, want));
            };
            field_types
                .iter()
                .map(|(name, typ)| {
                    let field = fields.get(name.as_ref()).unwrap_or(&JsonValue::Null);
                    Ok((name.clone(), json_to_rex(field, typ, ts)?))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(RexValue::Dict)
        }
    }
}

/// Convert an owned Rex value to JSON using its concrete Rex type.
pub fn rex_to_json(
    value: &RexValue,
    want: &Type,
    ts: &TypeSystem,
) -> Result<JsonValue, EngineError> {
    match want.as_ref() {
        TypeKind::Var(tv) => Err(error(format!(
            "cannot encode unresolved type variable t{} to JSON",
            tv.id
        ))),
        TypeKind::Con(con) => value_to_json_for_con(value, &con.name(), &[], ts),
        TypeKind::App(_, _) => {
            let (head, args) = decompose_type_app(want);
            let TypeKind::Con(con) = head.as_ref() else {
                return Err(error(format!("unsupported applied type {want}")));
            };
            value_to_json_for_con(value, &con.name(), &args, ts)
        }
        TypeKind::Fun(_, _) => Err(error("cannot encode function value to JSON".into())),
        TypeKind::Tuple(item_types) => {
            let RexValue::Tuple(items) = value else {
                return Err(type_mismatch_value(value, want));
            };
            if items.len() != item_types.len() {
                return Err(type_mismatch_value(value, want));
            }
            items
                .iter()
                .zip(item_types)
                .map(|(item, typ)| rex_to_json(item, typ, ts))
                .collect::<Result<Vec<_>, _>>()
                .map(JsonValue::Array)
        }
        TypeKind::Record(field_types) => {
            let RexValue::Dict(fields) = value else {
                return Err(type_mismatch_value(value, want));
            };
            if fields.len() != field_types.len() {
                return Err(type_mismatch_value(value, want));
            }
            field_types
                .iter()
                .map(|(name, typ)| {
                    let field = fields
                        .get(name)
                        .ok_or_else(|| type_mismatch_value(value, want))?;
                    Ok((name.to_string(), rex_to_json(field, typ, ts)?))
                })
                .collect::<Result<Map<_, _>, _>>()
                .map(JsonValue::Object)
        }
    }
}

fn json_to_value_for_con(
    json: &JsonValue,
    name: &Symbol,
    args: &[Type],
    ts: &TypeSystem,
) -> Result<RexValue, EngineError> {
    match (name.as_ref(), args) {
        ("bool", []) => match json {
            JsonValue::Bool(value) => Ok(RexValue::Bool(*value)),
            _ => Err(type_mismatch_json(
                json,
                &Type::builtin(BuiltinTypeId::Bool),
            )),
        },
        ("u8", []) => Ok(RexValue::U8(ranged_unsigned(json, "u8")?)),
        ("u16", []) => Ok(RexValue::U16(ranged_unsigned(json, "u16")?)),
        ("u32", []) => Ok(RexValue::U32(ranged_unsigned(json, "u32")?)),
        ("u64", []) => Ok(RexValue::U64(json_u64(json)?)),
        ("i8", []) => Ok(RexValue::I8(ranged_signed(json, "i8")?)),
        ("i16", []) => Ok(RexValue::I16(ranged_signed(json, "i16")?)),
        ("i32", []) => Ok(RexValue::I32(ranged_signed(json, "i32")?)),
        ("i64", []) => Ok(RexValue::I64(json_i64(json)?)),
        ("f32", []) => Ok(RexValue::F32(json_f64(json)? as f32)),
        ("f64", []) => Ok(RexValue::F64(json_f64(json)?)),
        ("string", []) => match json {
            JsonValue::String(value) => Ok(RexValue::String(value.clone())),
            _ => Err(type_mismatch_json(
                json,
                &Type::builtin(BuiltinTypeId::String),
            )),
        },
        ("uuid", []) => serde_json::from_value(json.clone())
            .map(RexValue::Uuid)
            .map_err(|json_error| error(format!("invalid uuid JSON: {json_error}"))),
        ("hash", []) => match json {
            JsonValue::String(value) => Hash::from_hex(value)
                .map(RexValue::Hash)
                .map_err(|hash_error| error(format!("invalid hash JSON: {hash_error}"))),
            _ => Err(type_mismatch_json(
                json,
                &Type::builtin(BuiltinTypeId::Hash),
            )),
        },
        ("datetime", []) => serde_json::from_value(json.clone())
            .map(RexValue::DateTime)
            .map_err(|json_error| error(format!("invalid datetime JSON: {json_error}"))),
        ("Option", [inner]) => match json {
            JsonValue::Null => Ok(RexValue::Adt(Symbol::intern("None"), vec![])),
            _ => Ok(RexValue::Adt(
                Symbol::intern("Some"),
                vec![json_to_rex(json, inner, ts)?],
            )),
        },
        ("Promise", [_]) => Ok(RexValue::Adt(
            Symbol::intern("Promise"),
            vec![json_to_rex(json, &Type::builtin(BuiltinTypeId::Uuid), ts)?],
        )),
        // Internal `Result` type argument order is error, then success.
        ("Result", [error_type, ok_type]) => {
            let JsonValue::Object(object) = json else {
                return Err(error(format!("expected result object JSON, got {json}")));
            };
            if object.len() != 1 {
                return Err(error(format!("expected result object JSON, got {json}")));
            }
            if let Some(value) = object.get("Ok") {
                Ok(RexValue::Adt(
                    Symbol::intern("Ok"),
                    vec![json_to_rex(value, ok_type, ts)?],
                ))
            } else if let Some(value) = object.get("Err") {
                Ok(RexValue::Adt(
                    Symbol::intern("Err"),
                    vec![json_to_rex(value, error_type, ts)?],
                ))
            } else {
                Err(error(format!(
                    "expected {{Ok:..}} or {{Err:..}}, got {json}"
                )))
            }
        }
        ("List", [element_type]) if is_u8_type(element_type) => {
            let JsonValue::Array(items) = json else {
                return Err(error(format!("expected array JSON for List, got {json}")));
            };
            items
                .iter()
                .map(|item| ranged_unsigned(item, "u8"))
                .collect::<Result<Vec<u8>, _>>()
                .map(RexValue::Bytes)
        }
        ("List", [element_type]) => {
            let JsonValue::Array(items) = json else {
                return Err(error(format!("expected array JSON for List, got {json}")));
            };
            items
                .iter()
                .map(|item| json_to_rex(item, element_type, ts))
                .collect::<Result<Vec<_>, _>>()
                .map(RexValue::List)
        }
        ("Dict", [element_type]) => {
            let JsonValue::Object(fields) = json else {
                return Err(error(format!("expected object JSON for Dict, got {json}")));
            };
            fields
                .iter()
                .map(|(name, value)| {
                    Ok((Symbol::intern(name), json_to_rex(value, element_type, ts)?))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(RexValue::Dict)
        }
        _ => json_to_value_for_adt(json, name, args, ts),
    }
}

fn value_to_json_for_con(
    value: &RexValue,
    name: &Symbol,
    args: &[Type],
    ts: &TypeSystem,
) -> Result<JsonValue, EngineError> {
    macro_rules! scalar {
        ($variant:ident, $map:expr) => {
            match value {
                RexValue::$variant(value) => Ok($map(*value)),
                _ => Err(type_mismatch_value(value, &named_type(name, args))),
            }
        };
    }
    match (name.as_ref(), args) {
        ("bool", []) => scalar!(Bool, JsonValue::Bool),
        ("u8", []) => scalar!(U8, |v| JsonValue::Number(u64::from(v).into())),
        ("u16", []) => scalar!(U16, |v| JsonValue::Number(u64::from(v).into())),
        ("u32", []) => scalar!(U32, |v| JsonValue::Number(u64::from(v).into())),
        ("u64", []) => scalar!(U64, |v: u64| JsonValue::Number(v.into())),
        ("i8", []) => scalar!(I8, |v| JsonValue::Number(i64::from(v).into())),
        ("i16", []) => scalar!(I16, |v| JsonValue::Number(i64::from(v).into())),
        ("i32", []) => scalar!(I32, |v| JsonValue::Number(i64::from(v).into())),
        ("i64", []) => scalar!(I64, |v: i64| JsonValue::Number(v.into())),
        ("f32", []) => scalar!(F32, |v| Number::from_f64(f64::from(v))
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null)),
        ("f64", []) => scalar!(F64, |v| Number::from_f64(v)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null)),
        ("string", []) => match value {
            RexValue::String(value) => Ok(JsonValue::String(value.clone())),
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("uuid", []) => match value {
            RexValue::Uuid(value) => serde_json::to_value(value)
                .map_err(|json_error| error(format!("failed to serialize uuid: {json_error}"))),
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("hash", []) => match value {
            RexValue::Hash(value) => Ok(JsonValue::String(value.to_hex().to_string())),
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("datetime", []) => match value {
            RexValue::DateTime(value) => serde_json::to_value(value)
                .map_err(|json_error| error(format!("failed to serialize datetime: {json_error}"))),
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("Option", [inner]) => match value {
            RexValue::Adt(tag, fields) if tag.as_ref() == "None" && fields.is_empty() => {
                Ok(JsonValue::Null)
            }
            RexValue::Adt(tag, fields) if tag.as_ref() == "Some" && fields.len() == 1 => {
                rex_to_json(&fields[0], inner, ts)
            }
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("Promise", [_]) => match value {
            RexValue::Adt(tag, fields) if tag.as_ref() == "Promise" && fields.len() == 1 => {
                rex_to_json(&fields[0], &Type::builtin(BuiltinTypeId::Uuid), ts)
            }
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("Result", [error_type, ok_type]) => match value {
            RexValue::Adt(tag, fields) if tag.as_ref() == "Ok" && fields.len() == 1 => {
                Ok(JsonValue::Object(Map::from_iter([(
                    "Ok".into(),
                    rex_to_json(&fields[0], ok_type, ts)?,
                )])))
            }
            RexValue::Adt(tag, fields) if tag.as_ref() == "Err" && fields.len() == 1 => {
                Ok(JsonValue::Object(Map::from_iter([(
                    "Err".into(),
                    rex_to_json(&fields[0], error_type, ts)?,
                )])))
            }
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("List", [element_type]) if is_u8_type(element_type) => match value {
            RexValue::Bytes(bytes) => Ok(JsonValue::Array(
                bytes
                    .iter()
                    .map(|byte| JsonValue::Number(u64::from(*byte).into()))
                    .collect(),
            )),
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("List", [element_type]) => match value {
            RexValue::List(items) => items
                .iter()
                .map(|item| rex_to_json(item, element_type, ts))
                .collect::<Result<Vec<_>, _>>()
                .map(JsonValue::Array),
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        ("Dict", [element_type]) => match value {
            RexValue::Dict(fields) => fields
                .iter()
                .map(|(name, value)| Ok((name.to_string(), rex_to_json(value, element_type, ts)?)))
                .collect::<Result<Map<_, _>, _>>()
                .map(JsonValue::Object),
            _ => Err(type_mismatch_value(value, &named_type(name, args))),
        },
        _ => value_to_json_for_adt(value, name, args, ts),
    }
}

fn json_to_value_for_adt(
    json: &JsonValue,
    adt_name: &Symbol,
    type_args: &[Type],
    ts: &TypeSystem,
) -> Result<RexValue, EngineError> {
    let adt = ts
        .adts
        .get(adt_name)
        .ok_or_else(|| error(format!("unknown ADT `{adt_name}`")))?;
    let substitutions = adt_subst(adt, type_args)?;
    if adt.variants.len() == 1 {
        let variant = &adt.variants[0];
        return decode_direct_variant(
            json,
            &variant.name,
            &instantiate_types(&variant.args, &substitutions),
            ts,
        );
    }
    let enum_like = adt.variants.iter().all(|variant| variant.args.is_empty());
    if let JsonValue::String(tag) = json {
        if let Some(variant) = adt
            .variants
            .iter()
            .find(|variant| variant.args.is_empty() && local_name_matches(&variant.name, tag))
        {
            return Ok(RexValue::Adt(variant.name.clone(), vec![]));
        }
        if enum_like {
            return Err(error(format!(
                "unknown enum tag `{tag}` for `{}`",
                adt.name
            )));
        }
    }
    if let JsonValue::Object(object) = json
        && object.len() == 1
        && let Some((tag, payload)) = object.iter().next()
        && let Some(variant) = adt
            .variants
            .iter()
            .find(|variant| local_name_matches(&variant.name, tag))
    {
        return decode_wrapped_variant(
            payload,
            &variant.name,
            &instantiate_types(&variant.args, &substitutions),
            ts,
        );
    }
    Err(error(format!(
        "expected ADT JSON representation for `{adt_name}`; got {json}"
    )))
}

fn value_to_json_for_adt(
    value: &RexValue,
    adt_name: &Symbol,
    type_args: &[Type],
    ts: &TypeSystem,
) -> Result<JsonValue, EngineError> {
    let adt = ts
        .adts
        .get(adt_name)
        .ok_or_else(|| error(format!("unknown ADT `{adt_name}`")))?;
    let substitutions = adt_subst(adt, type_args)?;
    let RexValue::Adt(tag, fields) = value else {
        return Err(type_mismatch_value(value, &named_type(adt_name, type_args)));
    };
    let variant = adt
        .variants
        .iter()
        .find(|variant| constructor_matches(tag, &variant.name))
        .ok_or_else(|| error(format!("constructor `{tag}` is not in ADT `{adt_name}`")))?;
    let field_types = instantiate_types(&variant.args, &substitutions);
    if fields.len() != field_types.len() {
        return Err(error(format!(
            "constructor `{tag}` expected {} args, got {}",
            field_types.len(),
            fields.len()
        )));
    }
    if adt.variants.len() == 1 {
        return encode_direct_variant(tag, fields, &field_types, ts);
    }
    if fields.is_empty() {
        return Ok(JsonValue::String(local_name(tag).to_owned()));
    }
    Ok(JsonValue::Object(Map::from_iter([(
        local_name(tag).to_owned(),
        encode_variant_fields(fields, &field_types, ts)?,
    )])))
}

fn decode_direct_variant(
    json: &JsonValue,
    constructor: &Symbol,
    field_types: &[Type],
    ts: &TypeSystem,
) -> Result<RexValue, EngineError> {
    let fields = match field_types {
        [] => match json {
            JsonValue::Null => vec![],
            JsonValue::String(tag) if tag == local_name(constructor) => vec![],
            _ => {
                return Err(error(format!(
                    "expected null or `{}` for unit constructor, got {json}",
                    local_name(constructor)
                )));
            }
        },
        [field_type] => vec![json_to_rex(json, field_type, ts)?],
        _ => decode_json_fields(json, field_types, ts)?,
    };
    Ok(RexValue::Adt(constructor.clone(), fields))
}

fn decode_wrapped_variant(
    payload: &JsonValue,
    constructor: &Symbol,
    field_types: &[Type],
    ts: &TypeSystem,
) -> Result<RexValue, EngineError> {
    let fields = match field_types {
        [] => vec![],
        [field_type] => vec![json_to_rex(payload, field_type, ts)?],
        _ => decode_json_fields(payload, field_types, ts)?,
    };
    Ok(RexValue::Adt(constructor.clone(), fields))
}

fn decode_json_fields(
    json: &JsonValue,
    field_types: &[Type],
    ts: &TypeSystem,
) -> Result<Vec<RexValue>, EngineError> {
    let JsonValue::Array(items) = json else {
        return Err(error(format!("expected array payload, got {json}")));
    };
    if items.len() != field_types.len() {
        return Err(error(format!("expected array payload, got {json}")));
    }
    items
        .iter()
        .zip(field_types)
        .map(|(item, typ)| json_to_rex(item, typ, ts))
        .collect()
}

fn encode_direct_variant(
    constructor: &Symbol,
    fields: &[RexValue],
    field_types: &[Type],
    ts: &TypeSystem,
) -> Result<JsonValue, EngineError> {
    match field_types {
        [] => Ok(JsonValue::String(local_name(constructor).to_owned())),
        [field_type] => rex_to_json(&fields[0], field_type, ts),
        _ => encode_variant_fields(fields, field_types, ts),
    }
}

fn encode_variant_fields(
    fields: &[RexValue],
    field_types: &[Type],
    ts: &TypeSystem,
) -> Result<JsonValue, EngineError> {
    fields
        .iter()
        .zip(field_types)
        .map(|(field, typ)| rex_to_json(field, typ, ts))
        .collect::<Result<Vec<_>, _>>()
        .map(JsonValue::Array)
}

fn decompose_type_app(typ: &Type) -> (Type, Vec<Type>) {
    let mut args = Vec::new();
    let mut head = typ.clone();
    while let TypeKind::App(function, argument) = head.as_ref() {
        args.push(argument.clone());
        head = function.clone();
    }
    args.reverse();
    (head, args)
}

fn named_type(name: &Symbol, args: &[Type]) -> Type {
    args.iter().fold(Type::con(name, args.len()), |head, arg| {
        Type::app(head, arg.clone())
    })
}

fn is_u8_type(typ: &Type) -> bool {
    matches!(typ.as_ref(), TypeKind::Con(con) if con.name().as_ref() == "u8")
}

fn adt_subst(adt: &AdtDecl, args: &[Type]) -> Result<BTreeMap<usize, Type>, EngineError> {
    if adt.params.len() != args.len() {
        return Err(error(format!(
            "ADT `{}` expects {} type args, got {}",
            adt.name,
            adt.params.len(),
            args.len()
        )));
    }
    Ok(adt
        .params
        .iter()
        .zip(args)
        .map(|(parameter, argument)| (parameter.var.id, argument.clone()))
        .collect())
}

fn instantiate_types(types: &[Type], substitutions: &BTreeMap<usize, Type>) -> Vec<Type> {
    types
        .iter()
        .map(|typ| instantiate_type(typ, substitutions))
        .collect()
}

fn instantiate_type(typ: &Type, substitutions: &BTreeMap<usize, Type>) -> Type {
    match typ.as_ref() {
        TypeKind::Var(variable) => substitutions
            .get(&variable.id)
            .cloned()
            .unwrap_or_else(|| typ.clone()),
        TypeKind::Con(_) => typ.clone(),
        TypeKind::App(function, argument) => Type::app(
            instantiate_type(function, substitutions),
            instantiate_type(argument, substitutions),
        ),
        TypeKind::Fun(argument, result) => Type::fun(
            instantiate_type(argument, substitutions),
            instantiate_type(result, substitutions),
        ),
        TypeKind::Tuple(items) => Type::tuple(
            items
                .iter()
                .map(|item| instantiate_type(item, substitutions)),
        ),
        TypeKind::Record(fields) => Type::record(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), instantiate_type(field, substitutions))),
        ),
    }
}

fn ranged_unsigned<T>(json: &JsonValue, name: &str) -> Result<T, EngineError>
where
    T: TryFrom<u64>,
{
    let value = json_u64(json)?;
    T::try_from(value).map_err(|_| error(format!("value {value} out of range for {name}")))
}

fn ranged_signed<T>(json: &JsonValue, name: &str) -> Result<T, EngineError>
where
    T: TryFrom<i64>,
{
    let value = json_i64(json)?;
    T::try_from(value).map_err(|_| error(format!("value {value} out of range for {name}")))
}

fn json_u64(json: &JsonValue) -> Result<u64, EngineError> {
    match json {
        JsonValue::Number(number) => number
            .as_u64()
            .ok_or_else(|| error(format!("expected unsigned integer JSON, got {json}"))),
        _ => Err(error(format!("expected unsigned integer JSON, got {json}"))),
    }
}

fn json_i64(json: &JsonValue) -> Result<i64, EngineError> {
    match json {
        JsonValue::Number(number) => number
            .as_i64()
            .ok_or_else(|| error(format!("expected signed integer JSON, got {json}"))),
        _ => Err(error(format!("expected signed integer JSON, got {json}"))),
    }
}

fn json_f64(json: &JsonValue) -> Result<f64, EngineError> {
    match json {
        JsonValue::Number(number) => number
            .as_f64()
            .ok_or_else(|| error(format!("expected floating-point JSON, got {json}"))),
        _ => Err(error(format!("expected floating-point JSON, got {json}"))),
    }
}

fn error(message: String) -> EngineError {
    EngineError::Custom(message)
}

fn type_mismatch_json(json: &JsonValue, want: &Type) -> EngineError {
    error(format!("JSON value {json} does not match Rex type {want}"))
}

fn type_mismatch_value(value: &RexValue, want: &Type) -> EngineError {
    error(format!(
        "Rex value of kind `{}` does not match Rex type {want}",
        value.value_type_name()
    ))
}

/// Convert a JSON object into owned values for a compiled `main` signature.
pub fn json_to_main_inputs(
    value: JsonValue,
    signature: &MainSignature,
    type_system: &TypeSystem,
) -> Result<BTreeMap<String, RexValue>, EngineError> {
    let JsonValue::Object(mut inputs) = value else {
        return Err(error(
            "input JSON must be an object whose fields are parameter names".into(),
        ));
    };
    let mut matched = Vec::with_capacity(signature.inputs().len());
    let mut missing = Vec::new();
    for input in signature.inputs() {
        match inputs.remove(&input.name) {
            Some(value) => matched.push((input.name.clone(), input.typ.clone(), value)),
            None => missing.push(input.name.clone()),
        }
    }
    let extra = inputs.into_iter().map(|(name, _)| name).collect::<Vec<_>>();
    if !missing.is_empty() || !extra.is_empty() {
        return Err(EngineError::MainInputMismatch { missing, extra });
    }
    matched
        .into_iter()
        .map(|(name, typ, value)| {
            let value = json_to_rex(&value, &typ, type_system).map_err(|error_value| {
                error(format!(
                    "failed to convert input `{name}` from JSON: {error_value}"
                ))
            })?;
            Ok((name, value))
        })
        .collect()
}
