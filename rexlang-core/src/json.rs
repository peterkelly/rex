use crate::{EngineError, Heap, Pointer};
use rexlang_ast::expr::{Symbol, sym};
use rexlang_typesystem::{AdtDecl, BuiltinTypeId, Type, TypeKind, TypeSystem};
use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;

fn local_name(name: &Symbol) -> &str {
    name.as_ref().rsplit('.').next().unwrap_or(name.as_ref())
}

fn local_name_matches(name: &Symbol, expected: &str) -> bool {
    local_name(name) == expected
}

fn runtime_ctor(name: &Symbol) -> Symbol {
    sym(local_name(name))
}

#[derive(Clone, Debug)]
pub struct EnumPatch {
    /// Variant name (kept as `enum_name` for backward compatibility).
    pub enum_name: String,
    pub discriminant: i64,
}

/// Options for handling enum JSON encodings.
///
/// Some enums are encoded as integers, while others are encoded as strings, and the type
/// information alone is not always sufficient to recover which encoding should be used. For this
/// reason, callers can whitelist ADT names that should be represented as integers in JSON. This
/// applies to both encoding and decoding.
///
/// Additionally, older generated type metadata may have missing/incorrect discriminants. To keep
/// JSON conversion behavior aligned with external serde-based encodings, callers can provide
/// per-variant integer patches via [`EnumPatch`].
#[derive(Clone, Default, Debug)]
pub struct JsonOptions {
    pub int_enums: BTreeMap<String, Vec<EnumPatch>>,
}

impl JsonOptions {
    /// Register the name of an ADT enum that should be encoded as integers, rather than strings.
    ///
    /// This only has an effect for unit variants.
    pub fn add_int_enum(&mut self, name: &str) {
        self.int_enums.insert(name.to_string(), vec![]);
    }

    /// Register an ADT enum for integer encoding, with explicit discriminant patches.
    ///
    /// This only has an effect for unit variants.
    pub fn add_int_enum_with_patches(&mut self, name: &str, patches: Vec<EnumPatch>) {
        self.int_enums.insert(name.to_string(), patches);
    }
}

/// Convert a JSON [`Value`] into a typed Rex runtime value (`Pointer`) allocated on `heap`.
///
/// The conversion is compatible with serde-style JSON representations for Rex-compatible Rust
/// types, but operates entirely via runtime Rex types (`Type`) plus the evaluator heap.
///
/// This is intended for cases where Rust compile-time types are not available and only runtime
/// type information exists.
///
/// Important behavior:
/// - JSON arrays targeting `Array a` become Rex runtime arrays (`Value::Array`).
/// - JSON arrays targeting tuple types become tuples.
/// - JSON arrays targeting `List a` become list ADTs (`Cons`/`Empty`).
pub fn json_to_rex(
    heap: &Heap,
    json: &Value,
    want: &Type,
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Pointer, EngineError> {
    match want.as_ref() {
        TypeKind::Var(tv) => Err(error(format!(
            "cannot decode JSON into unresolved type variable t{}",
            tv.id
        ))),
        TypeKind::Con(con) => json_to_pointer_for_con(heap, json, &con.name, &[], ts, opts),
        TypeKind::App(_, _) => {
            let (head, args) = decompose_type_app(want);
            if let TypeKind::Con(con) = head.as_ref() {
                json_to_pointer_for_con(heap, json, &con.name, &args, ts, opts)
            } else {
                Err(error(format!("unsupported applied type {}", want)))
            }
        }
        TypeKind::Fun(_, _) => Err(error("cannot decode JSON into function type".to_string())),
        TypeKind::Tuple(items) => match json {
            Value::Array(values) if values.len() == items.len() => {
                let mut out = Vec::with_capacity(values.len());
                for (value, item_ty) in values.iter().zip(items.iter()) {
                    out.push(json_to_rex(heap, value, item_ty, ts, opts)?);
                }
                heap.alloc_tuple(out)
            }
            _ => Err(type_mismatch_json(json, want)),
        },
        TypeKind::Record(fields) => match json {
            Value::Object(entries) => {
                let mut out = BTreeMap::new();
                for (k, t) in fields {
                    let j = entries.get(k.as_ref()).unwrap_or(&Value::Null);
                    out.insert(k.clone(), json_to_rex(heap, j, t, ts, opts)?);
                }
                heap.alloc_dict(out)
            }
            _ => Err(type_mismatch_json(json, want)),
        },
    }
}

/// Convert a typed Rex runtime value (`Pointer`) to a JSON [`Value`].
///
/// The conversion is compatible with serde-style JSON representations for Rex-compatible Rust
/// types, but operates on runtime values stored in the evaluator heap.
pub fn rex_to_json(
    heap: &Heap,
    pointer: &Pointer,
    want: &Type,
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Value, EngineError> {
    match want.as_ref() {
        TypeKind::Var(tv) => Err(error(format!(
            "cannot encode unresolved type variable t{} to JSON",
            tv.id
        ))),
        TypeKind::Con(con) => pointer_to_json_for_con(heap, pointer, &con.name, &[], ts, opts),
        TypeKind::App(_, _) => {
            let (head, args) = decompose_type_app(want);
            if let TypeKind::Con(con) = head.as_ref() {
                pointer_to_json_for_con(heap, pointer, &con.name, &args, ts, opts)
            } else {
                Err(error(format!("unsupported applied type {}", want)))
            }
        }
        TypeKind::Fun(_, _) => Err(error("cannot encode function value to JSON".to_string())),
        TypeKind::Tuple(item_types) => {
            let values = heap.pointer_as_tuple(pointer)?;
            if values.len() != item_types.len() {
                return Err(type_mismatch_pointer(heap, pointer, want));
            }
            let mut out = Vec::with_capacity(values.len());
            for (p, t) in values.iter().zip(item_types.iter()) {
                out.push(rex_to_json(heap, p, t, ts, opts)?);
            }
            Ok(Value::Array(out))
        }
        TypeKind::Record(fields) => {
            let entries = heap.pointer_as_dict(pointer)?;
            if entries.len() != fields.len() {
                return Err(type_mismatch_pointer(heap, pointer, want));
            }
            let mut out = Map::new();
            for (k, t) in fields {
                let p = entries
                    .get(k)
                    .ok_or_else(|| type_mismatch_pointer(heap, pointer, want))?;
                out.insert(k.to_string(), rex_to_json(heap, p, t, ts, opts)?);
            }
            Ok(Value::Object(out))
        }
    }
}

fn json_to_pointer_for_con(
    heap: &Heap,
    json: &Value,
    con_name: &Symbol,
    con_args: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Pointer, EngineError> {
    match (con_name.as_ref(), con_args) {
        ("bool", []) => match json {
            Value::Bool(v) => heap.alloc_bool(*v),
            _ => Err(type_mismatch_json(
                json,
                &Type::builtin(BuiltinTypeId::Bool),
            )),
        },

        ("u8", []) => {
            let v = json_u64(json)?;
            u8::try_from(v)
                .map_err(|_| error(format!("value {} out of range for u8", v)))
                .and_then(|x| heap.alloc_u8(x))
        }
        ("u16", []) => {
            let v = json_u64(json)?;
            u16::try_from(v)
                .map_err(|_| error(format!("value {} out of range for u16", v)))
                .and_then(|x| heap.alloc_u16(x))
        }
        ("u32", []) => {
            let v = json_u64(json)?;
            u32::try_from(v)
                .map_err(|_| error(format!("value {} out of range for u32", v)))
                .and_then(|x| heap.alloc_u32(x))
        }
        ("u64", []) => heap.alloc_u64(json_u64(json)?),

        ("i8", []) => {
            let v = json_i64(json)?;
            i8::try_from(v)
                .map_err(|_| error(format!("value {} out of range for i8", v)))
                .and_then(|x| heap.alloc_i8(x))
        }
        ("i16", []) => {
            let v = json_i64(json)?;
            i16::try_from(v)
                .map_err(|_| error(format!("value {} out of range for i16", v)))
                .and_then(|x| heap.alloc_i16(x))
        }
        ("i32", []) => {
            let v = json_i64(json)?;
            i32::try_from(v)
                .map_err(|_| error(format!("value {} out of range for i32", v)))
                .and_then(|x| heap.alloc_i32(x))
        }
        ("i64", []) => heap.alloc_i64(json_i64(json)?),

        ("f32", []) => heap.alloc_f32(json_f64(json)? as f32),
        ("f64", []) => heap.alloc_f64(json_f64(json)?),

        ("string", []) => match json {
            Value::String(s) => heap.alloc_string(s.clone()),
            _ => Err(type_mismatch_json(
                json,
                &Type::builtin(BuiltinTypeId::String),
            )),
        },
        ("uuid", []) => {
            let u = serde_json::from_value(json.clone())
                .map_err(|e| error(format!("invalid uuid JSON: {e}")))?;
            heap.alloc_uuid(u)
        }
        ("datetime", []) => {
            let dt = serde_json::from_value(json.clone())
                .map_err(|e| error(format!("invalid datetime JSON: {e}")))?;
            heap.alloc_datetime(dt)
        }

        ("Option", [inner]) => match json {
            Value::Null => heap.alloc_adt(sym("None"), vec![]),
            _ => {
                let inner_ptr = json_to_rex(heap, json, inner, ts, opts)?;
                heap.alloc_adt(sym("Some"), vec![inner_ptr])
            }
        },

        ("Promise", [_inner]) => {
            let promise_id =
                json_to_rex(heap, json, &Type::builtin(BuiltinTypeId::Uuid), ts, opts)?;
            heap.alloc_adt(sym("Promise"), vec![promise_id])
        }

        // Internal argument order is Result err ok.
        ("Result", [err_t, ok_t]) => match json {
            Value::Object(obj) if obj.len() == 1 => {
                if let Some(v) = obj.get("Ok") {
                    let p = json_to_rex(heap, v, ok_t, ts, opts)?;
                    heap.alloc_adt(sym("Ok"), vec![p])
                } else if let Some(v) = obj.get("Err") {
                    let p = json_to_rex(heap, v, err_t, ts, opts)?;
                    heap.alloc_adt(sym("Err"), vec![p])
                } else {
                    Err(error(format!(
                        "expected {{Ok:..}} or {{Err:..}}, got {}",
                        json
                    )))
                }
            }
            _ => Err(error(format!("expected result object JSON, got {}", json))),
        },

        // IMPORTANT: JSON arrays map to runtime arrays, not lists.
        ("Array", [elem_t]) => match json {
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(json_to_rex(heap, item, elem_t, ts, opts)?);
                }
                heap.alloc_array(out)
            }
            _ => Err(error(format!(
                "expected array JSON for Array, got {}",
                json
            ))),
        },

        ("List", [elem_t]) => match json {
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(json_to_rex(heap, item, elem_t, ts, opts)?);
                }
                let mut list = heap.alloc_adt(sym("Empty"), vec![])?;
                for p in out.into_iter().rev() {
                    list = heap.alloc_adt(sym("Cons"), vec![p, list])?;
                }
                Ok(list)
            }
            _ => Err(error(format!("expected array JSON for List, got {}", json))),
        },

        ("Dict", [elem_t]) => match json {
            Value::Object(obj) => {
                let mut out = BTreeMap::new();
                for (k, v) in obj {
                    out.insert(sym(k), json_to_rex(heap, v, elem_t, ts, opts)?);
                }
                heap.alloc_dict(out)
            }
            _ => Err(error(format!(
                "expected object JSON for Dict, got {}",
                json
            ))),
        },

        _ => json_to_pointer_for_adt(heap, json, con_name, con_args, ts, opts),
    }
}

fn pointer_to_json_for_con(
    heap: &Heap,
    pointer: &Pointer,
    con_name: &Symbol,
    con_args: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Value, EngineError> {
    match (con_name.as_ref(), con_args) {
        ("bool", []) => Ok(Value::Bool(heap.pointer_as_bool(pointer)?)),
        ("u8", []) => Ok(Value::Number(
            u64::from(heap.pointer_as_u8(pointer)?).into(),
        )),
        ("u16", []) => Ok(Value::Number(
            u64::from(heap.pointer_as_u16(pointer)?).into(),
        )),
        ("u32", []) => Ok(Value::Number(
            u64::from(heap.pointer_as_u32(pointer)?).into(),
        )),
        ("u64", []) => Ok(Value::Number(heap.pointer_as_u64(pointer)?.into())),
        ("i8", []) => Ok(Value::Number(
            i64::from(heap.pointer_as_i8(pointer)?).into(),
        )),
        ("i16", []) => Ok(Value::Number(
            i64::from(heap.pointer_as_i16(pointer)?).into(),
        )),
        ("i32", []) => Ok(Value::Number(
            i64::from(heap.pointer_as_i32(pointer)?).into(),
        )),
        ("i64", []) => Ok(Value::Number(heap.pointer_as_i64(pointer)?.into())),
        ("f32", []) => Number::from_f64(f64::from(heap.pointer_as_f32(pointer)?))
            .map(Value::Number)
            .ok_or_else(|| error("invalid f32 value for JSON".to_string())),
        ("f64", []) => Number::from_f64(heap.pointer_as_f64(pointer)?)
            .map(Value::Number)
            .ok_or_else(|| error("invalid f64 value for JSON".to_string())),
        ("string", []) => Ok(Value::String(heap.pointer_as_string(pointer)?)),
        ("uuid", []) => serde_json::to_value(heap.pointer_as_uuid(pointer)?)
            .map_err(|e| error(format!("failed to serialize uuid: {e}"))),
        ("datetime", []) => serde_json::to_value(heap.pointer_as_datetime(pointer)?)
            .map_err(|e| error(format!("failed to serialize datetime: {e}"))),

        ("Option", [inner_t]) => {
            let (tag, args) = heap.pointer_as_adt(pointer)?;
            match (tag.as_ref(), args.as_slice()) {
                ("None", []) => Ok(Value::Null),
                ("Some", [x]) => rex_to_json(heap, x, inner_t, ts, opts),
                _ => Err(type_mismatch_pointer(
                    heap,
                    pointer,
                    &Type::app(Type::builtin(BuiltinTypeId::Option), inner_t.clone()),
                )),
            }
        }

        ("Promise", [inner_t]) => {
            let (tag, args) = heap.pointer_as_adt(pointer)?;
            match (tag.as_ref(), args.as_slice()) {
                ("Promise", [promise_id]) => rex_to_json(
                    heap,
                    promise_id,
                    &Type::builtin(BuiltinTypeId::Uuid),
                    ts,
                    opts,
                ),
                _ => Err(type_mismatch_pointer(
                    heap,
                    pointer,
                    &Type::app(Type::builtin(BuiltinTypeId::Promise), inner_t.clone()),
                )),
            }
        }

        // Internal argument order is Result err ok.
        ("Result", [err_t, ok_t]) => {
            let (tag, args) = heap.pointer_as_adt(pointer)?;
            match (tag.as_ref(), args.as_slice()) {
                ("Ok", [x]) => {
                    let mut out = Map::new();
                    out.insert("Ok".to_string(), rex_to_json(heap, x, ok_t, ts, opts)?);
                    Ok(Value::Object(out))
                }
                ("Err", [x]) => {
                    let mut out = Map::new();
                    out.insert("Err".to_string(), rex_to_json(heap, x, err_t, ts, opts)?);
                    Ok(Value::Object(out))
                }
                _ => Err(type_mismatch_pointer(
                    heap,
                    pointer,
                    &Type::app(
                        Type::app(Type::builtin(BuiltinTypeId::Result), err_t.clone()),
                        ok_t.clone(),
                    ),
                )),
            }
        }

        ("Array", [elem_t]) => {
            let items = heap.pointer_as_array(pointer)?;
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                out.push(rex_to_json(heap, item, elem_t, ts, opts)?);
            }
            Ok(Value::Array(out))
        }

        ("List", [elem_t]) => {
            let items = list_to_vec(heap, pointer)?;
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                out.push(rex_to_json(heap, item, elem_t, ts, opts)?);
            }
            Ok(Value::Array(out))
        }

        ("Dict", [elem_t]) => {
            let entries = heap.pointer_as_dict(pointer)?;
            let mut out = Map::new();
            for (k, v) in &entries {
                out.insert(k.to_string(), rex_to_json(heap, v, elem_t, ts, opts)?);
            }
            Ok(Value::Object(out))
        }

        _ => pointer_to_json_for_adt(heap, pointer, con_name, con_args, ts, opts),
    }
}

fn json_to_pointer_for_adt(
    heap: &Heap,
    json: &Value,
    adt_name: &Symbol,
    type_args: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Pointer, EngineError> {
    let adt = ts
        .adts
        .get(adt_name)
        .ok_or_else(|| error(format!("unknown ADT `{}`", adt_name)))?;
    let subst = adt_subst(adt, type_args)?;

    if adt.variants.len() == 1 {
        let v = &adt.variants[0];
        let arg_types = instantiate_types(&v.args, &subst);
        return decode_direct_variant(heap, json, &v.name, &arg_types, ts, opts);
    }

    let enum_name = adt.name.to_string();
    let enum_like = adt.variants.iter().all(|v| v.args.is_empty());
    if enum_like {
        if opts.int_enums.contains_key(&enum_name) {
            if let Value::Number(n) = json
                && let Some(i) = n.as_i64()
            {
                for (idx, v) in adt.variants.iter().enumerate() {
                    if variant_discriminant(&enum_name, &v.name, idx, opts) == Some(i) {
                        return heap.alloc_adt(runtime_ctor(&v.name), vec![]);
                    }
                }
            }
            return Err(error(format!(
                "expected integer enum JSON for `{}`, got {}",
                enum_name, json
            )));
        }
        if let Value::String(tag) = json {
            if let Some(v) = adt
                .variants
                .iter()
                .find(|v| local_name_matches(&v.name, tag))
            {
                return heap.alloc_adt(runtime_ctor(&v.name), vec![]);
            }
            return Err(error(format!(
                "unknown enum tag `{}` for `{}`",
                tag, enum_name
            )));
        }
        return Err(error(format!(
            "expected enum string JSON for `{}`, got {}",
            enum_name, json
        )));
    }

    if let Value::String(tag) = json
        && let Some(v) = adt
            .variants
            .iter()
            .find(|v| v.args.is_empty() && local_name_matches(&v.name, tag))
    {
        return heap.alloc_adt(runtime_ctor(&v.name), vec![]);
    }

    if let Value::Object(obj) = json
        && obj.len() == 1
    {
        let Some((tag, payload)) = obj.iter().next() else {
            return Err(error(format!(
                "expected ADT JSON representation for `{}`; got {}",
                adt_name, json
            )));
        };
        if let Some(v) = adt
            .variants
            .iter()
            .find(|v| local_name_matches(&v.name, tag))
        {
            let arg_types = instantiate_types(&v.args, &subst);
            return decode_wrapped_variant(heap, payload, &v.name, &arg_types, ts, opts);
        }
    }

    Err(error(format!(
        "expected ADT JSON representation for `{}`; got {}",
        adt_name, json
    )))
}

fn pointer_to_json_for_adt(
    heap: &Heap,
    pointer: &Pointer,
    adt_name: &Symbol,
    type_args: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Value, EngineError> {
    let adt = ts
        .adts
        .get(adt_name)
        .ok_or_else(|| error(format!("unknown ADT `{}`", adt_name)))?;
    let subst = adt_subst(adt, type_args)?;

    let (tag, args) = heap.pointer_as_adt(pointer)?;
    let v = adt
        .variants
        .iter()
        .find(|v| local_name_matches(&v.name, local_name(&tag)))
        .ok_or_else(|| {
            error(format!(
                "constructor `{}` is not in ADT `{}`",
                tag, adt_name
            ))
        })?;
    let arg_types = instantiate_types(&v.args, &subst);
    if args.len() != arg_types.len() {
        return Err(error(format!(
            "constructor `{}` expected {} args, got {}",
            tag,
            arg_types.len(),
            args.len()
        )));
    }

    if adt.variants.len() == 1 {
        return encode_direct_variant(heap, &tag, &args, &arg_types, ts, opts);
    }

    let enum_name = adt.name.to_string();
    let enum_like = adt.variants.iter().all(|v| v.args.is_empty());
    if enum_like && args.is_empty() {
        if opts.int_enums.contains_key(&enum_name) {
            let idx = adt
                .variants
                .iter()
                .position(|v| local_name_matches(&v.name, local_name(&tag)))
                .ok_or_else(|| error(format!("missing enum variant `{}`", tag)))?;
            let d = variant_discriminant(&enum_name, &tag, idx, opts).ok_or_else(|| {
                error(format!(
                    "missing integer discriminant for enum `{}` variant `{}`",
                    enum_name, tag
                ))
            })?;
            return Ok(Value::Number(d.into()));
        }
        return Ok(Value::String(local_name(&tag).to_string()));
    }

    if args.is_empty() {
        return Ok(Value::String(local_name(&tag).to_string()));
    }

    let payload = encode_wrapped_variant(heap, &args, &arg_types, ts, opts)?;
    let mut out = Map::new();
    out.insert(local_name(&tag).to_string(), payload);
    Ok(Value::Object(out))
}

fn decode_direct_variant(
    heap: &Heap,
    json: &Value,
    ctor: &Symbol,
    arg_types: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Pointer, EngineError> {
    match arg_types {
        [] => match json {
            Value::Null => heap.alloc_adt(runtime_ctor(ctor), vec![]),
            Value::String(tag) if tag == local_name(ctor) => {
                heap.alloc_adt(runtime_ctor(ctor), vec![])
            }
            _ => Err(error(format!(
                "expected null or `{}` for unit constructor, got {}",
                local_name(ctor),
                json
            ))),
        },
        [t0] => {
            let p = json_to_rex(heap, json, t0, ts, opts)?;
            heap.alloc_adt(runtime_ctor(ctor), vec![p])
        }
        _ => match json {
            Value::Array(items) if items.len() == arg_types.len() => {
                let mut args = Vec::with_capacity(items.len());
                for (item, t) in items.iter().zip(arg_types.iter()) {
                    args.push(json_to_rex(heap, item, t, ts, opts)?);
                }
                heap.alloc_adt(runtime_ctor(ctor), args)
            }
            _ => Err(error(format!(
                "expected array payload for constructor `{}`, got {}",
                local_name(ctor),
                json
            ))),
        },
    }
}

fn decode_wrapped_variant(
    heap: &Heap,
    payload: &Value,
    ctor: &Symbol,
    arg_types: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Pointer, EngineError> {
    match arg_types {
        [] => heap.alloc_adt(runtime_ctor(ctor), vec![]),
        [t0] => {
            let p = json_to_rex(heap, payload, t0, ts, opts)?;
            heap.alloc_adt(runtime_ctor(ctor), vec![p])
        }
        _ => match payload {
            Value::Array(items) if items.len() == arg_types.len() => {
                let mut args = Vec::with_capacity(items.len());
                for (item, t) in items.iter().zip(arg_types.iter()) {
                    args.push(json_to_rex(heap, item, t, ts, opts)?);
                }
                heap.alloc_adt(runtime_ctor(ctor), args)
            }
            _ => Err(error(format!(
                "expected array payload for constructor `{}`, got {}",
                local_name(ctor),
                payload
            ))),
        },
    }
}

fn encode_direct_variant(
    heap: &Heap,
    ctor: &Symbol,
    args: &[Pointer],
    arg_types: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Value, EngineError> {
    match arg_types {
        [] => Ok(Value::String(local_name(ctor).to_string())),
        [t0] => rex_to_json(heap, &args[0], t0, ts, opts),
        _ => {
            let mut out = Vec::with_capacity(args.len());
            for (arg, t) in args.iter().zip(arg_types.iter()) {
                out.push(rex_to_json(heap, arg, t, ts, opts)?);
            }
            Ok(Value::Array(out))
        }
    }
}

fn encode_wrapped_variant(
    heap: &Heap,
    args: &[Pointer],
    arg_types: &[Type],
    ts: &TypeSystem,
    opts: &JsonOptions,
) -> Result<Value, EngineError> {
    match arg_types {
        [] => Ok(Value::Null),
        [t0] => rex_to_json(heap, &args[0], t0, ts, opts),
        _ => {
            let mut out = Vec::with_capacity(args.len());
            for (arg, t) in args.iter().zip(arg_types.iter()) {
                out.push(rex_to_json(heap, arg, t, ts, opts)?);
            }
            Ok(Value::Array(out))
        }
    }
}

fn decompose_type_app(typ: &Type) -> (Type, Vec<Type>) {
    let mut args = Vec::new();
    let mut cur = typ.clone();
    while let TypeKind::App(f, a) = cur.as_ref() {
        args.push(a.clone());
        cur = f.clone();
    }
    args.reverse();
    (cur, args)
}

fn adt_subst(adt: &AdtDecl, type_args: &[Type]) -> Result<BTreeMap<usize, Type>, EngineError> {
    if adt.params.len() != type_args.len() {
        return Err(error(format!(
            "ADT `{}` expects {} type args, got {}",
            adt.name,
            adt.params.len(),
            type_args.len()
        )));
    }
    let mut subst = BTreeMap::new();
    for (param, arg) in adt.params.iter().zip(type_args.iter()) {
        subst.insert(param.var.id, arg.clone());
    }
    Ok(subst)
}

fn instantiate_types(ts: &[Type], subst: &BTreeMap<usize, Type>) -> Vec<Type> {
    ts.iter().map(|t| instantiate_type(t, subst)).collect()
}

fn instantiate_type(t: &Type, subst: &BTreeMap<usize, Type>) -> Type {
    match t.as_ref() {
        TypeKind::Var(tv) => subst.get(&tv.id).cloned().unwrap_or_else(|| t.clone()),
        TypeKind::Con(_) => t.clone(),
        TypeKind::App(f, a) => Type::app(instantiate_type(f, subst), instantiate_type(a, subst)),
        TypeKind::Fun(a, b) => Type::fun(instantiate_type(a, subst), instantiate_type(b, subst)),
        TypeKind::Tuple(xs) => Type::tuple(xs.iter().map(|x| instantiate_type(x, subst)).collect()),
        TypeKind::Record(fields) => Type::record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), instantiate_type(v, subst)))
                .collect(),
        ),
    }
}

fn variant_discriminant(
    enum_type_name: &str,
    variant_name: &Symbol,
    idx: usize,
    opts: &JsonOptions,
) -> Option<i64> {
    let patches = opts.int_enums.get(enum_type_name)?;
    for p in patches {
        if p.enum_name == variant_name.as_ref() || p.enum_name == local_name(variant_name) {
            return Some(p.discriminant);
        }
    }
    Some(idx as i64)
}

fn list_to_vec(heap: &Heap, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
    let mut out = Vec::new();
    let mut cur = *pointer;
    loop {
        let (tag, args) = heap.pointer_as_adt(&cur)?;
        if tag.as_ref() == "Empty" && args.is_empty() {
            return Ok(out);
        }
        if tag.as_ref() == "Cons" && args.len() == 2 {
            out.push(args[0]);
            cur = args[1];
            continue;
        }
        return Err(error(format!(
            "expected list ADT chain (Cons/Empty), found constructor `{}`",
            tag
        )));
    }
}

fn json_u64(json: &Value) -> Result<u64, EngineError> {
    match json {
        Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| error(format!("expected unsigned integer JSON, got {}", json))),
        _ => Err(error(format!(
            "expected unsigned integer JSON, got {}",
            json
        ))),
    }
}

fn json_i64(json: &Value) -> Result<i64, EngineError> {
    match json {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| error(format!("expected signed integer JSON, got {}", json))),
        _ => Err(error(format!("expected signed integer JSON, got {}", json))),
    }
}

fn json_f64(json: &Value) -> Result<f64, EngineError> {
    match json {
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| error(format!("expected floating-point JSON, got {}", json))),
        _ => Err(error(format!("expected floating-point JSON, got {}", json))),
    }
}

fn error(msg: String) -> EngineError {
    EngineError::Custom(msg)
}

fn type_mismatch_json(json: &Value, want: &Type) -> EngineError {
    error(format!(
        "JSON value {} does not match Rex type {}",
        json, want
    ))
}

fn type_mismatch_pointer(heap: &Heap, pointer: &Pointer, want: &Type) -> EngineError {
    match heap.type_name(pointer) {
        Ok(got) => error(format!(
            "Rex value of runtime kind `{}` does not match Rex type {}",
            got, want
        )),
        Err(e) => e,
    }
}
