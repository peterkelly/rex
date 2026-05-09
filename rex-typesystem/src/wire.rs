//! Stable JSON-facing representations of Rex types.
//!
//! The internal type structures intentionally optimize for inference and
//! evaluation. This module provides an explicit wire format for external tools
//! that need to persist or inspect type information without depending on those
//! internal details.

use crate::{
    error::{CollectAdtsError, TypeError},
    types::{
        AdtDecl, AdtParam, AdtVariant, BuiltinTypeId, Predicate, Scheme, Type, TypeConst, TypeKind,
        TypeVar, TypeVarId, Types, collect_adts_in_types, order_adt_family,
    },
    typesystem::{TypeSystem, TypeVarSupply},
};
use rex_ast::Symbol;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const TYPE_BUNDLE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeBundle {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub types: BTreeMap<String, WireScheme>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adts: Vec<WireAdtDecl>,
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
pub struct WireField {
    pub name: String,
    #[serde(rename = "type")]
    pub typ: WireType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireAdtDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<WireAdtVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireAdtVariant {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<WireType>,
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
    pub fn from_schemes<I, K>(schemes: I, type_system: &TypeSystem) -> Result<Self, TypeError>
    where
        I: IntoIterator<Item = (K, Scheme)>,
        K: Into<String>,
    {
        let mut wire_schemes = BTreeMap::new();
        let mut referenced_types = Vec::new();

        for (name, scheme) in schemes {
            referenced_types.push(scheme.typ.clone());
            referenced_types.extend(scheme.preds.iter().map(|pred| pred.typ.clone()));

            let name = name.into();
            let wire_scheme = WireScheme::try_from_scheme(&scheme)?;
            if wire_schemes.insert(name.clone(), wire_scheme).is_some() {
                return Err(wire_error(format!("duplicate exported type name `{name}`")));
            }
        }

        let adts = collect_wire_adts_for_types(&referenced_types, type_system)?;
        Ok(Self {
            schema_version: TYPE_BUNDLE_SCHEMA_VERSION,
            types: wire_schemes,
            adts,
        })
    }

    pub fn into_parts(self) -> Result<(Vec<AdtDecl>, BTreeMap<String, Scheme>), TypeError> {
        if self.schema_version != TYPE_BUNDLE_SCHEMA_VERSION {
            return Err(wire_error(format!(
                "unsupported type bundle schema version {}; expected {}",
                self.schema_version, TYPE_BUNDLE_SCHEMA_VERSION
            )));
        }

        let mut supply = TypeVarSupply::new();
        let adts = self
            .adts
            .iter()
            .map(|adt| adt.to_adt_decl_with_supply(&mut supply))
            .collect::<Result<Vec<_>, _>>()?;
        let adts = order_adt_family(adts)?;

        let mut schemes = BTreeMap::new();
        for (name, scheme) in self.types {
            if schemes
                .insert(name.clone(), scheme.to_scheme_with_supply(&mut supply)?)
                .is_some()
            {
                return Err(wire_error(format!("duplicate exported type name `{name}`")));
            }
        }

        Ok((adts, schemes))
    }

    pub fn register_into(
        self,
        type_system: &mut TypeSystem,
    ) -> Result<BTreeMap<String, Scheme>, TypeError> {
        let (adts, schemes) = self.into_parts()?;
        for adt in adts {
            type_system.register_adt(&adt);
        }
        Ok(schemes)
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
                free.extend(arg.ftv());
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
            params.push(name);
        }
        let variants = adt
            .variants
            .iter()
            .map(|variant| WireAdtVariant {
                name: variant.name.to_string(),
                args: variant
                    .args
                    .iter()
                    .map(|arg| WireType::from_type_with_namer(arg, &mut namer))
                    .collect(),
            })
            .collect();

        Ok(Self {
            name: adt.name.to_string(),
            params,
            variants,
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
            validate_name("ADT parameter", param)?;
            if vars_by_name.contains_key(param) {
                return Err(wire_error(format!(
                    "duplicate type parameter `{param}` in ADT `{}`",
                    self.name
                )));
            }
            let var = supply.fresh(Some(Symbol::intern(param)));
            vars_by_name.insert(param.clone(), var.clone());
            params.push(AdtParam {
                name: Symbol::intern(param),
                var,
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
                .map(|arg| arg.to_type_with_ctx(&mut ctx))
                .collect::<Result<Vec<_>, _>>()?;
            variants.push(AdtVariant {
                name: Symbol::intern(&variant.name),
                args,
            });
        }

        Ok(AdtDecl {
            name: Symbol::intern(&self.name),
            params,
            variants,
        })
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
            .flat_map(|variant| variant.args.iter().cloned())
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
