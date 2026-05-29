use std::collections::BTreeMap;

use rex_typesystem::{
    types::{Scheme, Type, TypeKind, TypeVar},
    typesystem::TypeSystem,
    wire::TypeBundle,
};
use serde::{Deserialize, Serialize};

use crate::EngineError;

/// Externally visible type signature of a compiled Rex program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainSignature {
    /// Main inputs in application order.
    pub inputs: Vec<MainInputSpec>,
    /// Result type after all inputs have been applied.
    pub result_type: Type,
}

/// One named input accepted by a compiled Rex program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainInputSpec {
    /// External input name.
    pub name: String,
    /// Rex type expected for this input.
    pub typ: Type,
}

/// JSON-serializable description of a compiled Rex program's external types.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub type_bundle: TypeBundle,
}

impl MainSignature {
    pub(crate) fn new(inputs: Vec<MainInputSpec>, result_type: Type) -> Self {
        Self {
            inputs,
            result_type,
        }
    }

    /// Main inputs in application order.
    pub fn inputs(&self) -> &[MainInputSpec] {
        &self.inputs
    }

    /// Result type after all inputs have been applied.
    pub fn result_type(&self) -> &Type {
        &self.result_type
    }

    /// Build a manifest containing all main input types and the result type.
    pub fn manifest(&self, type_system: &TypeSystem) -> Result<Manifest, EngineError> {
        build_manifest(
            self.inputs
                .iter()
                .map(|input| (input.name.as_str(), &input.typ)),
            &self.result_type,
            type_system,
        )
    }
}

/// Build a manifest from named input types plus a result type.
pub fn build_manifest<'a, I>(
    inputs: I,
    result_type: &Type,
    type_system: &TypeSystem,
) -> Result<Manifest, EngineError>
where
    I: IntoIterator<Item = (&'a str, &'a Type)>,
{
    let mut schemes = Vec::new();

    for (name, typ) in inputs {
        schemes.push((format!("input.{name}"), scheme_for_type(typ.clone())));
    }

    schemes.push(("result".to_string(), scheme_for_type(result_type.clone())));
    let type_bundle = TypeBundle::from_schemes(schemes, type_system).map_err(EngineError::Type)?;

    Ok(Manifest {
        name: None,
        description: None,
        type_bundle,
    })
}

/// Return true when a type still contains type variables.
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
