use crate::error::EngineError;
use rex_ast::{NameRef, Span, Symbol, TypeExpr};
use rex_typesystem::{
    error::{CollectAdtsError, TypeError},
    types::{BuiltinTypeId, Scheme, Type, TypeKind},
};

pub(crate) fn type_expr_from_type(typ: &Type) -> TypeExpr {
    match typ.as_ref() {
        TypeKind::Var(tv) => {
            let name = tv
                .name
                .clone()
                .unwrap_or_else(|| Symbol::intern(&format!("t{}", tv.id)));
            TypeExpr::Name(Span::default(), NameRef::Unqualified(name))
        }
        TypeKind::Con(con) => TypeExpr::Name(Span::default(), NameRef::Unqualified(con.name())),
        TypeKind::App(fun, arg) => {
            if let TypeKind::App(head, err) = fun.as_ref()
                && let TypeKind::Con(con) = head.as_ref()
                && con.is_builtin(BuiltinTypeId::Result)
                && con.arity() == 2
            {
                let result = TypeExpr::Name(Span::default(), NameRef::Unqualified(con.name()));
                let ok_expr = type_expr_from_type(arg);
                let err_expr = type_expr_from_type(err);
                let app1 = TypeExpr::App(Span::default(), Box::new(result), Box::new(ok_expr));
                return TypeExpr::App(Span::default(), Box::new(app1), Box::new(err_expr));
            }
            TypeExpr::App(
                Span::default(),
                Box::new(type_expr_from_type(fun)),
                Box::new(type_expr_from_type(arg)),
            )
        }
        TypeKind::Fun(arg, ret) => TypeExpr::Fun(
            Span::default(),
            Box::new(type_expr_from_type(arg)),
            Box::new(type_expr_from_type(ret)),
        ),
        TypeKind::Tuple(elems) => TypeExpr::Tuple(
            Span::default(),
            elems.iter().map(type_expr_from_type).collect(),
        ),
        TypeKind::Record(fields) => TypeExpr::Record(
            Span::default(),
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), type_expr_from_type(ty)))
                .collect(),
        ),
    }
}

/// Convert ADT collection conflicts into an embedder-facing `EngineError`.
///
/// # Examples
///
/// ```rust,ignore
/// use rex_engine::collect_adts_error_to_engine;
/// use rex_typesystem::{collect_adts_in_types, Type};
///
/// let err = collect_adts_in_types(vec![
///     Type::user_con("Thing", 1),
///     Type::user_con("Thing", 2),
/// ])
/// .unwrap_err();
///
/// let engine_err = collect_adts_error_to_engine(err);
/// assert!(engine_err.to_string().contains("conflicting ADT definitions"));
/// ```
pub fn collect_adts_error_to_engine(err: CollectAdtsError) -> EngineError {
    let details = err
        .conflicts
        .into_iter()
        .map(|conflict| {
            let defs = conflict
                .definitions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: [{defs}]", conflict.name)
        })
        .collect::<Vec<_>>()
        .join("; ");
    EngineError::Custom(format!(
        "conflicting ADT definitions discovered in input types: {details}"
    ))
}

pub(crate) fn adt_family_error_to_engine(err: TypeError) -> EngineError {
    match err {
        TypeError::Internal(message) => EngineError::Custom(message),
        other => EngineError::Type(other),
    }
}

fn native_export_arg_types(
    scheme: &Scheme,
    arity: usize,
) -> Result<(Vec<Type>, Type), EngineError> {
    let mut args = Vec::with_capacity(arity);
    let mut rest = scheme.typ.clone();
    for _ in 0..arity {
        let Some((arg, tail)) = split_fun(&rest) else {
            return Err(EngineError::Internal(format!(
                "native export type `{}` does not accept {arity} argument(s)",
                scheme.typ
            )));
        };
        args.push(arg);
        rest = tail;
    }
    Ok((args, rest))
}

pub(crate) fn validate_native_export_scheme(
    scheme: &Scheme,
    arity: usize,
) -> Result<(), EngineError> {
    let _ = native_export_arg_types(scheme, arity)?;
    Ok(())
}

pub(crate) fn validate_host_value_export_scheme(
    scheme: &Scheme,
    arity: usize,
) -> Result<(), EngineError> {
    let (args, result) = native_export_arg_types(scheme, arity)?;
    if args.iter().any(type_contains_function) || type_contains_function(&result) {
        return Err(EngineError::Custom(format!(
            "host export type `{}` contains a function value that cannot cross the owned Value boundary",
            scheme.typ
        )));
    }
    Ok(())
}

fn type_contains_function(typ: &Type) -> bool {
    match typ.as_ref() {
        TypeKind::Fun(_, _) => true,
        TypeKind::App(function, argument) => {
            type_contains_function(function) || type_contains_function(argument)
        }
        TypeKind::Tuple(items) => items.iter().any(type_contains_function),
        TypeKind::Record(fields) => fields
            .iter()
            .any(|(_, field)| type_contains_function(field)),
        TypeKind::Var(_) | TypeKind::Con(_) => false,
    }
}

pub(crate) fn normalize_name(name: &str) -> Symbol {
    if let Some(stripped) = name.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        let ok = stripped
            .chars()
            .all(|c| !c.is_alphanumeric() && c != '_' && !c.is_whitespace());
        if ok {
            return Symbol::intern(stripped);
        }
    }
    Symbol::intern(name)
}

pub(crate) fn is_function_type(typ: &Type) -> bool {
    matches!(typ.as_ref(), TypeKind::Fun(..))
}

pub(crate) fn type_arity(typ: &Type) -> usize {
    let mut count = 0;
    let mut cur = typ;
    while let TypeKind::Fun(_, next) = cur.as_ref() {
        count += 1;
        cur = next;
    }
    count
}

pub(crate) fn split_fun(typ: &Type) -> Option<(Type, Type)> {
    match typ.as_ref() {
        TypeKind::Fun(a, b) => Some((a.clone(), b.clone())),
        _ => None,
    }
}
