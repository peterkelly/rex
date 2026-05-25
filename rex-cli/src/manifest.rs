use std::collections::BTreeMap;

use rex::typesystem::{Scheme, Type, TypeBundle, TypeKind, TypeSystem, TypeVar};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub type_bundle: TypeBundle,
}

pub fn build_manifest<'a, I>(
    inputs: I,
    result_type: &Type,
    type_system: &TypeSystem,
) -> Result<Manifest, String>
where
    I: IntoIterator<Item = (&'a str, &'a Type)>,
{
    let mut schemes = Vec::new();

    for (name, typ) in inputs {
        schemes.push((format!("input.{name}"), scheme_for_type(typ.clone())));
    }

    schemes.push(("result".to_string(), scheme_for_type(result_type.clone())));
    let type_bundle = TypeBundle::from_schemes(schemes, type_system)
        .map_err(|e| format!("failed to build type manifest: {e}"))?;

    Ok(Manifest {
        name: None,
        description: None,
        type_bundle,
    })
}

pub fn type_has_vars(typ: &Type) -> bool {
    !collect_type_vars(typ).is_empty()
}

fn scheme_for_type(typ: Type) -> Scheme {
    let vars = collect_type_vars(&typ).into_values().collect();
    Scheme::new(vars, vec![], typ)
}

fn collect_type_vars(typ: &Type) -> BTreeMap<usize, TypeVar> {
    let mut out = BTreeMap::new();
    collect_type_vars_inner(typ, &mut out);
    out
}

fn collect_type_vars_inner(typ: &Type, out: &mut BTreeMap<usize, TypeVar>) {
    match typ.as_ref() {
        TypeKind::Var(tv) => {
            out.entry(tv.id).or_insert_with(|| tv.clone());
        }
        TypeKind::Con(_) => {}
        TypeKind::App(fun, arg) | TypeKind::Fun(fun, arg) => {
            collect_type_vars_inner(fun, out);
            collect_type_vars_inner(arg, out);
        }
        TypeKind::Tuple(items) => {
            for item in items {
                collect_type_vars_inner(item, out);
            }
        }
        TypeKind::Record(fields) => {
            for (_, typ) in fields {
                collect_type_vars_inner(typ, out);
            }
        }
    }
}
