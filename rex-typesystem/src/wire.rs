//! Stable JSON-facing representations of Rex types.
//!
//! The internal type structures intentionally optimize for inference and
//! evaluation. This module provides an explicit wire format for external tools
//! that need to persist or inspect type information without depending on those
//! internal details.

use crate::{
    error::{CollectAdtsError, TypeError},
    types::{
        AdtArgument, AdtDecl, AdtField, AdtParam, AdtVariant, BuiltinTypeId, Predicate,
        RegisteredValue, Scheme, Type, TypeConst, TypeKind, TypeVar, TypeVarId, Types,
        collect_adts_in_types, order_adt_family,
    },
    typesystem::{TypeSystem, TypeVarSupply},
};
use rex_ast::Symbol;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A persistable collection of registered values and the ADTs referenced by their types.
///
/// Documentation is stored alongside the declaration it describes. The format intentionally has
/// no schema-version field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeBundle {
    /// Optional Markdown documentation for the bundle as a whole.
    ///
    /// A bundle used to persist a virtual module can store that module's documentation here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    /// Registered values and function overloads, grouped by Rex name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Vec<WireValueDecl>>,
    /// Documented declarations for the user-defined ADTs referenced by `values`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adts: Vec<WireAdtDecl>,
}

/// A JSON-facing registered value or function overload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireValueDecl {
    /// The overload's polymorphic type scheme.
    pub scheme: WireScheme,
    /// Rex-visible function parameter names, in application order.
    ///
    /// Parameters have names only; per-parameter documentation is not represented.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
    /// Markdown API documentation for this overload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

/// Registered value overloads grouped by Rex name.
pub type RegisteredValueMap = BTreeMap<String, Vec<RegisteredValue>>;

/// A decoded bundle containing semantic declarations and their API documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedTypeBundle {
    /// Optional Markdown documentation for the bundle as a whole.
    pub docs: Option<String>,
    /// Dependency-ordered ADT declarations referenced by the bundle's values.
    pub adts: Vec<AdtDecl>,
    /// Registered values and function overloads, grouped by Rex name.
    pub values: RegisteredValueMap,
}

/// A decoded bundle after its ADTs have been installed in a type system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredTypeBundle {
    /// Optional Markdown documentation for the bundle as a whole.
    pub docs: Option<String>,
    /// Registered values and function overloads, grouped by Rex name.
    pub values: RegisteredValueMap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireScheme {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<WireTypeVar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<WirePredicate>,
    #[serde(rename = "type")]
    pub typ: WireType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTypeVar {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePredicate {
    pub class: String,
    #[serde(rename = "type")]
    pub typ: WireType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WireType {
    Var {
        name: String,
    },
    Named {
        name: String,
        arity: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<WireType>,
    },
    Builtin {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<WireType>,
    },
    App {
        fun: Box<WireType>,
        arg: Box<WireType>,
    },
    Fun {
        params: Vec<WireType>,
        ret: Box<WireType>,
    },
    Tuple {
        items: Vec<WireType>,
    },
    Record {
        fields: Vec<WireField>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// A named record field in the wire type model.
pub struct WireField {
    /// The Rex-visible field name.
    pub name: String,
    /// The field's Rex type.
    #[serde(rename = "type")]
    pub typ: WireType,
    /// Markdown API documentation for this field.
    ///
    /// Documentation is valid when this field belongs directly to a
    /// [`WireAdtArg::Record`]. A structural [`WireType::Record`] cannot preserve field
    /// documentation when decoded into the semantic [`Type`] representation and rejects it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// A JSON-facing algebraic data type declaration.
pub struct WireAdtDecl {
    /// The Rex-visible type constructor name.
    pub name: String,
    /// The declaration's documented type parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<WireAdtParam>,
    /// The declaration's documented constructor variants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<WireAdtVariant>,
    /// Markdown API documentation for this ADT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// A JSON-facing ADT type parameter.
pub struct WireAdtParam {
    /// The Rex-visible parameter name.
    pub name: String,
    /// Markdown API documentation for this parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// A JSON-facing ADT constructor variant.
pub struct WireAdtVariant {
    /// The Rex-visible constructor name.
    pub name: String,
    /// The constructor arguments, in application order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<WireAdtArg>,
    /// Markdown API documentation for this variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
/// A JSON-facing ADT constructor argument.
pub enum WireAdtArg {
    /// One positional constructor argument.
    Positional {
        /// The argument's Rex type.
        #[serde(rename = "type")]
        typ: WireType,
        /// Markdown API documentation for this argument.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        docs: Option<String>,
    },
    /// One record-shaped constructor argument.
    Record {
        /// The record's documented fields.
        fields: Vec<WireField>,
        /// Markdown API documentation for the record argument as a whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        docs: Option<String>,
    },
}

impl From<&Type> for WireType {
    fn from(value: &Type) -> Self {
        WireType::from_type(value)
    }
}

impl TryFrom<WireType> for Type {
    type Error = TypeError;

    fn try_from(value: WireType) -> Result<Self, Self::Error> {
        value.to_type()
    }
}

impl TryFrom<&WireType> for Type {
    type Error = TypeError;

    fn try_from(value: &WireType) -> Result<Self, Self::Error> {
        value.to_type()
    }
}

impl TryFrom<&Scheme> for WireScheme {
    type Error = TypeError;

    fn try_from(value: &Scheme) -> Result<Self, Self::Error> {
        WireScheme::try_from_scheme(value)
    }
}

impl TryFrom<WireScheme> for Scheme {
    type Error = TypeError;

    fn try_from(value: WireScheme) -> Result<Self, Self::Error> {
        value.to_scheme()
    }
}

impl TryFrom<&WireScheme> for Scheme {
    type Error = TypeError;

    fn try_from(value: &WireScheme) -> Result<Self, Self::Error> {
        value.to_scheme()
    }
}

impl TryFrom<&AdtDecl> for WireAdtDecl {
    type Error = TypeError;

    fn try_from(value: &AdtDecl) -> Result<Self, Self::Error> {
        WireAdtDecl::try_from_adt_decl(value)
    }
}

impl TryFrom<WireAdtDecl> for AdtDecl {
    type Error = TypeError;

    fn try_from(value: WireAdtDecl) -> Result<Self, Self::Error> {
        value.to_adt_decl()
    }
}

impl TryFrom<&WireAdtDecl> for AdtDecl {
    type Error = TypeError;

    fn try_from(value: &WireAdtDecl) -> Result<Self, Self::Error> {
        value.to_adt_decl()
    }
}

impl TypeBundle {
    /// Build a bundle from schemes without value documentation or source parameter names.
    ///
    /// Function parameters receive generated names such as `arg0`. Bundle-level docs remain
    /// unset; attach them with [`TypeBundle::with_docs`].
    pub fn from_schemes<I, K>(schemes: I, type_system: &TypeSystem) -> Result<Self, TypeError>
    where
        I: IntoIterator<Item = (K, Scheme)>,
        K: Into<String>,
    {
        Self::from_registered_values(
            schemes.into_iter().map(|(name, scheme)| {
                let params = decompose_fun_type(&scheme.typ)
                    .0
                    .iter()
                    .enumerate()
                    .map(|(index, _)| Symbol::intern(&format!("arg{index}")))
                    .collect();
                (
                    name,
                    vec![RegisteredValue {
                        scheme,
                        params,
                        docs: None,
                    }],
                )
            }),
            type_system,
        )
    }

    /// Build a bundle while preserving each registered value's docs and parameter names.
    ///
    /// Bundle-level docs remain unset; attach them with [`TypeBundle::with_docs`]. Referenced ADT
    /// documentation is copied from `type_system`.
    pub fn from_registered_values<I, K>(
        values: I,
        type_system: &TypeSystem,
    ) -> Result<Self, TypeError>
    where
        I: IntoIterator<Item = (K, Vec<RegisteredValue>)>,
        K: Into<String>,
    {
        let mut wire_values = BTreeMap::new();
        let mut referenced_types = Vec::new();
        for (name, declarations) in values {
            let name = name.into();
            if declarations.is_empty() {
                return Err(wire_error(format!(
                    "exported value `{name}` has no declarations"
                )));
            }
            let mut wire_declarations = Vec::with_capacity(declarations.len());
            for declaration in declarations {
                referenced_types.push(declaration.scheme.typ.clone());
                referenced_types.extend(
                    declaration
                        .scheme
                        .preds
                        .iter()
                        .map(|predicate| predicate.typ.clone()),
                );
                wire_declarations.push(WireValueDecl {
                    scheme: WireScheme::try_from_scheme(&declaration.scheme)?,
                    params: declaration
                        .params
                        .into_iter()
                        .map(|param| param.to_string())
                        .collect(),
                    docs: declaration.docs,
                });
            }
            if wire_values
                .insert(name.clone(), wire_declarations)
                .is_some()
            {
                return Err(wire_error(format!(
                    "duplicate exported value name `{name}`"
                )));
            }
        }

        let adts = collect_wire_adts_for_types(&referenced_types, type_system)?;
        Ok(Self {
            docs: None,
            values: wire_values,
            adts,
        })
    }

    /// Attach Markdown documentation to this bundle.
    pub fn with_docs(mut self, docs: impl Into<String>) -> Self {
        self.docs = Some(docs.into());
        self
    }

    /// Decode the bundle into its semantic ADTs and registered values.
    pub fn into_parts(self) -> Result<DecodedTypeBundle, TypeError> {
        let TypeBundle {
            docs,
            values: wire_values,
            adts: wire_adts,
        } = self;
        let mut supply = TypeVarSupply::new();
        let adts = wire_adts
            .iter()
            .map(|adt| adt.to_adt_decl_with_supply(&mut supply))
            .collect::<Result<Vec<_>, _>>()?;
        let adts = order_adt_family(adts)?;

        let mut values = BTreeMap::new();
        for (name, declarations) in wire_values {
            if declarations.is_empty() {
                return Err(wire_error(format!(
                    "exported value `{name}` has no declarations"
                )));
            }
            let mut decoded = Vec::with_capacity(declarations.len());
            for declaration in declarations {
                let scheme = declaration.scheme.to_scheme_with_supply(&mut supply)?;
                let expected_params = decompose_fun_type(&scheme.typ).0.len();
                if !declaration.params.is_empty() && declaration.params.len() != expected_params {
                    return Err(wire_error(format!(
                        "exported value `{name}` has {} parameters but its type has {expected_params}",
                        declaration.params.len()
                    )));
                }
                let params = declaration
                    .params
                    .into_iter()
                    .map(|param| {
                        validate_name("value parameter", &param)?;
                        Ok(Symbol::intern(&param))
                    })
                    .collect::<Result<Vec<_>, TypeError>>()?;
                decoded.push(RegisteredValue {
                    scheme,
                    params,
                    docs: declaration.docs,
                });
            }
            values.insert(name, decoded);
        }

        Ok(DecodedTypeBundle { docs, adts, values })
    }

    /// Decode the bundle, register its ADTs, and return its docs and values.
    pub fn register_into(
        self,
        type_system: &mut TypeSystem,
    ) -> Result<RegisteredTypeBundle, TypeError> {
        let DecodedTypeBundle { docs, adts, values } = self.into_parts()?;
        for adt in adts {
            type_system.register_adt(&adt)?;
        }
        Ok(RegisteredTypeBundle { docs, values })
    }
}

impl WireScheme {
    pub fn try_from_scheme(scheme: &Scheme) -> Result<Self, TypeError> {
        let bound = scheme
            .vars
            .iter()
            .map(|var| var.id)
            .collect::<BTreeSet<_>>();
        let mut free = scheme.typ.ftv();
        for pred in &scheme.preds {
            free.extend(pred.typ.ftv());
        }
        let unbound = free.difference(&bound).copied().collect::<Vec<TypeVarId>>();
        if !unbound.is_empty() {
            return Err(wire_error(format!(
                "scheme contains unquantified type variable ids {unbound:?}"
            )));
        }

        let mut namer = TypeVarNamer::default();
        let vars = scheme
            .vars
            .iter()
            .map(|var| WireTypeVar {
                name: namer.name_for(var),
            })
            .collect();
        let constraints = scheme
            .preds
            .iter()
            .map(|pred| WirePredicate {
                class: pred.class.to_string(),
                typ: WireType::from_type_with_namer(&pred.typ, &mut namer),
            })
            .collect();
        let typ = WireType::from_type_with_namer(&scheme.typ, &mut namer);

        Ok(Self {
            vars,
            constraints,
            typ,
        })
    }

    pub fn to_scheme(&self) -> Result<Scheme, TypeError> {
        let mut supply = TypeVarSupply::new();
        self.to_scheme_with_supply(&mut supply)
    }

    fn to_scheme_with_supply(&self, supply: &mut TypeVarSupply) -> Result<Scheme, TypeError> {
        let mut vars_by_name = BTreeMap::new();
        let mut vars = Vec::new();
        for var in &self.vars {
            validate_name("type variable", &var.name)?;
            if vars_by_name.contains_key(&var.name) {
                return Err(wire_error(format!(
                    "duplicate type variable `{}` in scheme",
                    var.name
                )));
            }
            let tv = supply.fresh(Some(Symbol::intern(&var.name)));
            vars_by_name.insert(var.name.clone(), tv.clone());
            vars.push(tv);
        }

        let mut ctx = TypeDecodeCtx {
            supply,
            vars_by_name,
            allow_free_vars: false,
        };
        let constraints = self
            .constraints
            .iter()
            .map(|pred| pred.to_predicate_with_ctx(&mut ctx))
            .collect::<Result<Vec<_>, _>>()?;
        let typ = self.typ.to_type_with_ctx(&mut ctx)?;
        Ok(Scheme::new(vars, constraints, typ))
    }
}

impl WirePredicate {
    fn to_predicate_with_ctx(&self, ctx: &mut TypeDecodeCtx<'_>) -> Result<Predicate, TypeError> {
        validate_name("class", &self.class)?;
        Ok(Predicate::new(&self.class, self.typ.to_type_with_ctx(ctx)?))
    }
}

impl WireType {
    pub fn from_type(typ: &Type) -> Self {
        let mut namer = TypeVarNamer::default();
        Self::from_type_with_namer(typ, &mut namer)
    }

    pub fn to_type(&self) -> Result<Type, TypeError> {
        let mut supply = TypeVarSupply::new();
        let mut ctx = TypeDecodeCtx {
            supply: &mut supply,
            vars_by_name: BTreeMap::new(),
            allow_free_vars: true,
        };
        self.to_type_with_ctx(&mut ctx)
    }

    fn from_type_with_namer(typ: &Type, namer: &mut TypeVarNamer) -> Self {
        match typ.as_ref() {
            TypeKind::Var(tv) => WireType::Var {
                name: namer.name_for(tv),
            },
            TypeKind::Con(con) => type_from_const(con, Vec::new()),
            TypeKind::App(fun, arg) => {
                if let Some((con, args)) = decompose_type_app(typ)
                    && args.len() <= con.arity()
                {
                    let args = wire_args_for_constructor(&con, args)
                        .into_iter()
                        .map(|arg| WireType::from_type_with_namer(&arg, namer))
                        .collect();
                    type_from_const(&con, args)
                } else {
                    WireType::App {
                        fun: Box::new(WireType::from_type_with_namer(fun, namer)),
                        arg: Box::new(WireType::from_type_with_namer(arg, namer)),
                    }
                }
            }
            TypeKind::Fun(_, _) => {
                let (params, ret) = decompose_fun_type(typ);
                WireType::Fun {
                    params: params
                        .iter()
                        .map(|param| WireType::from_type_with_namer(param, namer))
                        .collect(),
                    ret: Box::new(WireType::from_type_with_namer(&ret, namer)),
                }
            }
            TypeKind::Tuple(items) => WireType::Tuple {
                items: items
                    .iter()
                    .map(|item| WireType::from_type_with_namer(item, namer))
                    .collect(),
            },
            TypeKind::Record(fields) => WireType::Record {
                fields: fields
                    .iter()
                    .map(|(name, typ)| WireField {
                        name: name.to_string(),
                        typ: WireType::from_type_with_namer(typ, namer),
                        docs: None,
                    })
                    .collect(),
            },
        }
    }

    fn to_type_with_ctx(&self, ctx: &mut TypeDecodeCtx<'_>) -> Result<Type, TypeError> {
        match self {
            WireType::Var { name } => {
                validate_name("type variable", name)?;
                ctx.type_var(name)
            }
            WireType::Named { name, arity, args } => named_type_to_type(name, *arity, args, ctx),
            WireType::Builtin { name, args } => builtin_type_to_type(name, args, ctx),
            WireType::App { fun, arg } => {
                let fun = fun.to_type_with_ctx(ctx)?;
                let arg = arg.to_type_with_ctx(ctx)?;
                Ok(Type::app(fun, arg))
            }
            WireType::Fun { params, ret } => {
                let mut typ = ret.to_type_with_ctx(ctx)?;
                for param in params.iter().rev() {
                    typ = Type::fun(param.to_type_with_ctx(ctx)?, typ);
                }
                Ok(typ)
            }
            WireType::Tuple { items } => Ok(Type::tuple(
                items
                    .iter()
                    .map(|item| item.to_type_with_ctx(ctx))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            WireType::Record { fields } => {
                let mut seen = BTreeSet::new();
                let mut out = Vec::new();
                for field in fields {
                    if field.docs.is_some() {
                        return Err(wire_error(format!(
                            "structural record field `{}` cannot carry documentation",
                            field.name
                        )));
                    }
                    validate_name("record field", &field.name)?;
                    if !seen.insert(field.name.clone()) {
                        return Err(wire_error(format!(
                            "duplicate record field `{}`",
                            field.name
                        )));
                    }
                    out.push((
                        Symbol::intern(&field.name),
                        field.typ.to_type_with_ctx(ctx)?,
                    ));
                }
                Ok(Type::record(out))
            }
        }
    }
}

impl WireAdtDecl {
    pub fn try_from_adt_decl(adt: &AdtDecl) -> Result<Self, TypeError> {
        let bound = adt
            .params
            .iter()
            .map(|param| param.var.id)
            .collect::<BTreeSet<_>>();
        let mut free = BTreeSet::new();
        for variant in &adt.variants {
            for arg in &variant.args {
                free.extend(arg.typ().ftv());
            }
        }
        let unbound = free.difference(&bound).copied().collect::<Vec<TypeVarId>>();
        if !unbound.is_empty() {
            return Err(wire_error(format!(
                "ADT `{}` contains unbound type variable ids {unbound:?}",
                adt.name
            )));
        }

        let mut namer = TypeVarNamer::default();
        let mut seen_params = BTreeSet::new();
        let mut params = Vec::new();
        for param in &adt.params {
            let name = param.name.to_string();
            if !seen_params.insert(name.clone()) {
                return Err(wire_error(format!(
                    "duplicate type parameter `{name}` in ADT `{}`",
                    adt.name
                )));
            }
            namer.bind_exact(&param.var, &name)?;
            params.push(WireAdtParam {
                name,
                docs: param.docs.clone(),
            });
        }
        let mut variants = Vec::with_capacity(adt.variants.len());
        for variant in &adt.variants {
            let mut args = Vec::with_capacity(variant.args.len());
            for arg in &variant.args {
                args.push(match arg {
                    AdtArgument::Positional { typ, docs } => WireAdtArg::Positional {
                        typ: WireType::from_type_with_namer(typ, &mut namer),
                        docs: docs.clone(),
                    },
                    AdtArgument::Record { fields, docs } => WireAdtArg::Record {
                        fields: fields
                            .iter()
                            .map(|field| WireField {
                                name: field.name.to_string(),
                                typ: WireType::from_type_with_namer(&field.typ, &mut namer),
                                docs: field.docs.clone(),
                            })
                            .collect(),
                        docs: docs.clone(),
                    },
                });
            }
            variants.push(WireAdtVariant {
                name: variant.name.to_string(),
                args,
                docs: variant.docs.clone(),
            });
        }

        Ok(Self {
            name: adt.name.to_string(),
            params,
            variants,
            docs: adt.docs.clone(),
        })
    }

    pub fn to_adt_decl(&self) -> Result<AdtDecl, TypeError> {
        let mut supply = TypeVarSupply::new();
        self.to_adt_decl_with_supply(&mut supply)
    }

    fn to_adt_decl_with_supply(&self, supply: &mut TypeVarSupply) -> Result<AdtDecl, TypeError> {
        validate_name("ADT", &self.name)?;
        if BuiltinTypeId::from_name(&self.name).is_some() {
            return Err(TypeError::ReservedTypeName(Symbol::intern(&self.name)));
        }

        let mut vars_by_name = BTreeMap::new();
        let mut params = Vec::new();
        for param in &self.params {
            validate_name("ADT parameter", &param.name)?;
            if vars_by_name.contains_key(&param.name) {
                return Err(wire_error(format!(
                    "duplicate type parameter `{}` in ADT `{}`",
                    param.name, self.name
                )));
            }
            let var = supply.fresh(Some(Symbol::intern(&param.name)));
            vars_by_name.insert(param.name.clone(), var.clone());
            params.push(AdtParam {
                name: Symbol::intern(&param.name),
                var,
                docs: param.docs.clone(),
            });
        }

        let mut ctx = TypeDecodeCtx {
            supply,
            vars_by_name,
            allow_free_vars: false,
        };
        let mut seen_variants = BTreeSet::new();
        let mut variants = Vec::new();
        for variant in &self.variants {
            validate_name("ADT variant", &variant.name)?;
            if !seen_variants.insert(variant.name.clone()) {
                return Err(wire_error(format!(
                    "duplicate variant `{}` in ADT `{}`",
                    variant.name, self.name
                )));
            }
            let args = variant
                .args
                .iter()
                .map(|arg| arg.to_adt_argument_with_ctx(&mut ctx))
                .collect::<Result<Vec<_>, _>>()?;
            variants.push(AdtVariant {
                name: Symbol::intern(&variant.name),
                args,
                docs: variant.docs.clone(),
            });
        }

        Ok(AdtDecl {
            name: Symbol::intern(&self.name),
            params,
            variants,
            docs: self.docs.clone(),
        })
    }
}

impl WireAdtArg {
    fn to_adt_argument_with_ctx(
        &self,
        ctx: &mut TypeDecodeCtx<'_>,
    ) -> Result<AdtArgument, TypeError> {
        match self {
            Self::Positional { typ, docs } => Ok(AdtArgument::Positional {
                typ: typ.to_type_with_ctx(ctx)?,
                docs: docs.clone(),
            }),
            Self::Record { fields, docs } => {
                let mut decoded = Vec::with_capacity(fields.len());
                let mut seen = BTreeSet::new();
                for field in fields {
                    validate_name("record field", &field.name)?;
                    if !seen.insert(field.name.clone()) {
                        return Err(wire_error(format!(
                            "duplicate record field `{}`",
                            field.name
                        )));
                    }
                    decoded.push(AdtField {
                        name: Symbol::intern(&field.name),
                        typ: field.typ.to_type_with_ctx(ctx)?,
                        docs: field.docs.clone(),
                    });
                }
                Ok(AdtArgument::Record {
                    fields: decoded,
                    docs: docs.clone(),
                })
            }
        }
    }
}

#[derive(Default)]
struct TypeVarNamer {
    by_id: BTreeMap<TypeVarId, String>,
    used: BTreeSet<String>,
}

impl TypeVarNamer {
    fn bind_exact(&mut self, var: &TypeVar, name: &str) -> Result<(), TypeError> {
        if let Some(existing) = self.by_id.get(&var.id) {
            if existing == name {
                return Ok(());
            }
            return Err(wire_error(format!(
                "type variable id {} is named both `{existing}` and `{name}`",
                var.id
            )));
        }
        if !self.used.insert(name.to_string()) {
            return Err(wire_error(format!("duplicate type variable name `{name}`")));
        }
        self.by_id.insert(var.id, name.to_string());
        Ok(())
    }

    fn name_for(&mut self, var: &TypeVar) -> String {
        if let Some(name) = self.by_id.get(&var.id) {
            return name.clone();
        }

        let base = var
            .name
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("t{}", var.id));
        let name = self.unique_name(base, var.id);
        self.by_id.insert(var.id, name.clone());
        name
    }

    fn unique_name(&mut self, base: String, id: TypeVarId) -> String {
        if self.used.insert(base.clone()) {
            return base;
        }

        let candidate = format!("{base}{id}");
        if self.used.insert(candidate.clone()) {
            return candidate;
        }

        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}{id}_{idx}");
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
            idx += 1;
        }
    }
}

struct TypeDecodeCtx<'a> {
    supply: &'a mut TypeVarSupply,
    vars_by_name: BTreeMap<String, TypeVar>,
    allow_free_vars: bool,
}

impl TypeDecodeCtx<'_> {
    fn type_var(&mut self, name: &str) -> Result<Type, TypeError> {
        if let Some(var) = self.vars_by_name.get(name) {
            return Ok(Type::var(var.clone()));
        }
        if !self.allow_free_vars {
            return Err(wire_error(format!("unknown type variable `{name}`")));
        }
        let var = self.supply.fresh(Some(Symbol::intern(name)));
        self.vars_by_name.insert(name.to_string(), var.clone());
        Ok(Type::var(var))
    }
}

fn collect_wire_adts_for_types(
    types: &[Type],
    type_system: &TypeSystem,
) -> Result<Vec<WireAdtDecl>, TypeError> {
    let adts = collect_adt_decls_for_types(types, type_system)?;
    adts.iter().map(WireAdtDecl::try_from_adt_decl).collect()
}

fn collect_adt_decls_for_types(
    types: &[Type],
    type_system: &TypeSystem,
) -> Result<Vec<AdtDecl>, TypeError> {
    let mut queue = VecDeque::new();
    for typ in collect_adts_in_types(types.to_vec()).map_err(collect_adts_error_to_type)? {
        if let TypeKind::Con(con) = typ.as_ref()
            && let Some(name) = con.user_name()
        {
            queue.push_back(name.clone());
        }
    }

    let mut seen = BTreeSet::new();
    let mut decls = Vec::new();
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }

        let adt = type_system
            .adts
            .get(&name)
            .ok_or_else(|| TypeError::UnknownTypeName(name.clone()))?
            .clone();
        let field_types = adt
            .variants
            .iter()
            .flat_map(|variant| variant.args.iter().map(AdtArgument::typ))
            .collect::<Vec<_>>();
        for dep in collect_adts_in_types(field_types).map_err(collect_adts_error_to_type)? {
            if let TypeKind::Con(con) = dep.as_ref()
                && let Some(name) = con.user_name()
            {
                queue.push_back(name.clone());
            }
        }
        decls.push(adt);
    }

    order_adt_family(decls)
}

fn collect_adts_error_to_type(err: CollectAdtsError) -> TypeError {
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
    wire_error(format!(
        "conflicting ADT definitions discovered in input types: {details}"
    ))
}

fn type_from_const(con: &TypeConst, args: Vec<WireType>) -> WireType {
    match con {
        TypeConst::Builtin(_) => WireType::Builtin {
            name: con.name_str().to_string(),
            args,
        },
        TypeConst::User { .. } => WireType::Named {
            name: con.name_str().to_string(),
            arity: con.arity(),
            args,
        },
    }
}

fn named_type_to_type(
    name: &str,
    arity: usize,
    args: &[WireType],
    ctx: &mut TypeDecodeCtx<'_>,
) -> Result<Type, TypeError> {
    validate_name("type constructor", name)?;
    if args.len() > arity {
        return Err(wire_error(format!(
            "type constructor `{name}` has arity {arity} but got {} argument(s)",
            args.len()
        )));
    }

    let decoded_args = args
        .iter()
        .map(|arg| arg.to_type_with_ctx(ctx))
        .collect::<Result<Vec<_>, _>>()?;

    if BuiltinTypeId::from_name(name).is_some() {
        return Err(wire_error(format!(
            "builtin type `{name}` must use the builtin wire kind"
        )));
    }

    let mut out = Type::user_con(name, arity);
    for arg in decoded_args {
        out = Type::app(out, arg);
    }
    Ok(out)
}

fn builtin_type_to_type(
    name: &str,
    args: &[WireType],
    ctx: &mut TypeDecodeCtx<'_>,
) -> Result<Type, TypeError> {
    validate_name("builtin type constructor", name)?;
    let id = BuiltinTypeId::from_name(name)
        .ok_or_else(|| wire_error(format!("unknown builtin type `{name}`")))?;
    let arity = id.arity();
    if args.len() > arity {
        return Err(wire_error(format!(
            "type constructor `{name}` has arity {arity} but got {} argument(s)",
            args.len()
        )));
    }

    let decoded_args = args
        .iter()
        .map(|arg| arg.to_type_with_ctx(ctx))
        .collect::<Result<Vec<_>, _>>()?;
    if id == BuiltinTypeId::Result && decoded_args.len() == 2 {
        return Ok(Type::result(
            decoded_args[0].clone(),
            decoded_args[1].clone(),
        ));
    }

    let mut out = Type::builtin(id);
    for arg in decoded_args {
        out = Type::app(out, arg);
    }
    Ok(out)
}

fn wire_args_for_constructor(con: &TypeConst, mut args: Vec<Type>) -> Vec<Type> {
    if con.is_builtin(BuiltinTypeId::Result) && args.len() == 2 {
        args.swap(0, 1);
    }
    args
}

fn decompose_type_app(typ: &Type) -> Option<(TypeConst, Vec<Type>)> {
    let mut args = Vec::new();
    let mut head = typ;
    while let TypeKind::App(fun, arg) = head.as_ref() {
        args.push(arg.clone());
        head = fun;
    }
    args.reverse();

    let TypeKind::Con(con) = head.as_ref() else {
        return None;
    };
    Some((con.clone(), args))
}

fn decompose_fun_type(typ: &Type) -> (Vec<Type>, Type) {
    let mut params = Vec::new();
    let mut cur = typ.clone();
    while let TypeKind::Fun(arg, ret) = cur.as_ref() {
        params.push(arg.clone());
        cur = ret.clone();
    }
    (params, cur)
}

fn validate_name(kind: &str, name: &str) -> Result<(), TypeError> {
    if name.trim().is_empty() {
        return Err(wire_error(format!("{kind} name cannot be empty")));
    }
    Ok(())
}

fn wire_error(message: String) -> TypeError {
    TypeError::Internal(format!("invalid type wire format: {message}"))
}
