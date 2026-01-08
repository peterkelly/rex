use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::BoxFuture;
use rex_ast::expr::{
    Decl, DeclareFnDecl, Expr, FnDecl, ImportDecl, ImportPath, InstanceDecl, Pattern, Program,
    Symbol, TypeConstraint, TypeDecl, TypeExpr, Var, intern,
};
use rex_gas::GasMeter;
use rex_lexer::Token;
use rex_parser::Parser as RexParser;
use rex_ts::{Predicate, Type};
use uuid::Uuid;

use crate::{Engine, EngineError, Value};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleId {
    Local { path: PathBuf, hash: String },
    Remote(String),
    Virtual(String),
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleId::Local { path, hash } => write!(f, "file:{}#{hash}", path.display()),
            ModuleId::Remote(url) => write!(f, "{url}"),
            ModuleId::Virtual(name) => write!(f, "virtual:{name}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolveRequest {
    pub module_name: String,
    pub importer: Option<ModuleId>,
}

#[derive(Clone, Debug)]
pub struct ResolvedModule {
    pub id: ModuleId,
    pub source: String,
}

type ResolverFuture = BoxFuture<'static, Result<Option<ResolvedModule>, EngineError>>;
type ResolverFn = Arc<dyn Fn(ResolveRequest) -> ResolverFuture + Send + Sync>;

#[derive(Clone)]
struct ResolverEntry {
    name: String,
    resolver: ResolverFn,
}

#[derive(Clone)]
pub struct ModuleExports {
    pub values: HashMap<Symbol, Symbol>,
    pub types: HashMap<Symbol, Symbol>,
    pub classes: HashMap<Symbol, Symbol>,
}

#[derive(Clone, Default)]
pub struct ReplState {
    alias_exports: HashMap<Symbol, ModuleExports>,
    defined_values: HashSet<Symbol>,
    importer_path: Option<PathBuf>,
}

impl ReplState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_importer_path(path: impl AsRef<Path>) -> Self {
        Self {
            importer_path: Some(path.as_ref().to_path_buf()),
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub struct ModuleInstance {
    pub id: ModuleId,
    pub exports: ModuleExports,
    pub init_value: Value,
}

#[derive(Default)]
struct ModuleState {
    loaded: HashMap<ModuleId, ModuleInstance>,
    loading: HashSet<ModuleId>,
}

fn sha256_hex(input: &[u8]) -> String {
    // Minimal SHA-256 implementation (no external deps) for stable content hashing.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h0: u32 = 0x6a09e667;
    let mut h1: u32 = 0xbb67ae85;
    let mut h2: u32 = 0x3c6ef372;
    let mut h3: u32 = 0xa54ff53a;
    let mut h4: u32 = 0x510e527f;
    let mut h5: u32 = 0x9b05688c;
    let mut h6: u32 = 0x1f83d9ab;
    let mut h7: u32 = 0x5be0cd19;

    let bit_len = (input.len() as u64) * 8;
    let mut msg = Vec::with_capacity(((input.len() + 9 + 63) / 64) * 64);
    msg.extend_from_slice(input);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let b = &chunk[i * 4..i * 4 + 4];
            *word = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;
        let mut f = h5;
        let mut g = h6;
        let mut h = h7;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
        h5 = h5.wrapping_add(f);
        h6 = h6.wrapping_add(g);
        h7 = h7.wrapping_add(h);
    }

    let out = [
        h0.to_be_bytes(),
        h1.to_be_bytes(),
        h2.to_be_bytes(),
        h3.to_be_bytes(),
        h4.to_be_bytes(),
        h5.to_be_bytes(),
        h6.to_be_bytes(),
        h7.to_be_bytes(),
    ]
    .concat();

    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[derive(Clone, Default)]
pub(crate) struct ModuleSystem {
    resolvers: Vec<ResolverEntry>,
    state: Arc<Mutex<ModuleState>>,
}

impl ModuleSystem {
    pub(crate) fn add_resolver(&mut self, name: impl Into<String>, resolver: ResolverFn) {
        self.resolvers.push(ResolverEntry {
            name: name.into(),
            resolver,
        });
    }

    pub(crate) fn resolve(&self, req: ResolveRequest) -> Result<ResolvedModule, EngineError> {
        for entry in &self.resolvers {
            tracing::trace!(resolver = %entry.name, module = %req.module_name, "trying module resolver");
            let fut = (entry.resolver)(ResolveRequest {
                module_name: req.module_name.clone(),
                importer: req.importer.clone(),
            });
            match futures::executor::block_on(fut)? {
                Some(resolved) => return Ok(resolved),
                None => continue,
            }
        }
        Err(EngineError::Module(format!(
            "module not found: {}",
            req.module_name
        )))
    }

    pub(crate) fn cached(&self, id: &ModuleId) -> Option<ModuleInstance> {
        self.state.lock().ok()?.loaded.get(id).cloned()
    }

    pub(crate) fn mark_loading(&self, id: &ModuleId) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("module state poisoned".into()))?;
        if state.loaded.contains_key(id) {
            return Ok(());
        }
        if state.loading.contains(id) {
            return Err(EngineError::Module(format!("cyclic module import: {id}")));
        }
        state.loading.insert(id.clone());
        Ok(())
    }

    pub(crate) fn store_loaded(&self, inst: ModuleInstance) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("module state poisoned".into()))?;
        state.loading.remove(&inst.id);
        state.loaded.insert(inst.id.clone(), inst);
        Ok(())
    }
}

fn prefix_for_module(id: &ModuleId) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.to_string().hash(&mut hasher);
    let h = hasher.finish();
    format!("@m{h:016x}")
}

fn qualify(prefix: &str, name: &Symbol) -> Symbol {
    intern(&format!("{prefix}.{}", name.as_ref()))
}

pub fn virtual_export_name(module: &str, export: &str) -> String {
    let id = ModuleId::Virtual(module.to_string());
    let prefix = prefix_for_module(&id);
    format!("{prefix}.{export}")
}

fn import_specifier(path: &ImportPath) -> String {
    match path {
        ImportPath::Local { segments, sha } => {
            let base = segments
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(".");
            if let Some(sha) = sha {
                format!("{base}#{sha}")
            } else {
                base
            }
        }
        ImportPath::Remote { url, sha } => {
            if let Some(sha) = sha {
                format!("{url}#{sha}")
            } else {
                url.clone()
            }
        }
    }
}

fn collect_local_renames(
    program: &Program,
    prefix: &str,
) -> (
    HashMap<Symbol, Symbol>,
    HashMap<Symbol, Symbol>,
    HashMap<Symbol, Symbol>,
) {
    let mut values = HashMap::new();
    let mut types = HashMap::new();
    let mut classes = HashMap::new();

    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) => {
                values.insert(fd.name.name.clone(), qualify(prefix, &fd.name.name));
            }
            Decl::DeclareFn(df) => {
                values.insert(df.name.name.clone(), qualify(prefix, &df.name.name));
            }
            Decl::Type(td) => {
                types.insert(td.name.clone(), qualify(prefix, &td.name));
                for variant in &td.variants {
                    values.insert(variant.name.clone(), qualify(prefix, &variant.name));
                }
            }
            Decl::Class(cd) => {
                classes.insert(cd.name.clone(), qualify(prefix, &cd.name));
            }
            Decl::Instance(..) | Decl::Import(..) => {}
        }
    }

    (values, types, classes)
}

fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<Symbol>) {
    match pat {
        Pattern::Wildcard(..) => {}
        Pattern::Var(v) => out.push(v.name.clone()),
        Pattern::Named(_, _, args) => {
            for arg in args {
                collect_pattern_bindings(arg, out);
            }
        }
        Pattern::Tuple(_, elems) | Pattern::List(_, elems) => {
            for elem in elems {
                collect_pattern_bindings(elem, out);
            }
        }
        Pattern::Cons(_, head, tail) => {
            collect_pattern_bindings(head, out);
            collect_pattern_bindings(tail, out);
        }
        Pattern::Dict(_, fields) => {
            for (_, pat) in fields {
                collect_pattern_bindings(pat, out);
            }
        }
    }
}

fn rename_type_expr(
    ty: &TypeExpr,
    type_renames: &HashMap<Symbol, Symbol>,
    class_renames: &HashMap<Symbol, Symbol>,
) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => {
            if let Some(new) = type_renames.get(name) {
                TypeExpr::Name(*span, new.clone())
            } else if let Some(new) = class_renames.get(name) {
                TypeExpr::Name(*span, new.clone())
            } else {
                TypeExpr::Name(*span, name.clone())
            }
        }
        TypeExpr::App(span, f, x) => TypeExpr::App(
            *span,
            Box::new(rename_type_expr(f, type_renames, class_renames)),
            Box::new(rename_type_expr(x, type_renames, class_renames)),
        ),
        TypeExpr::Fun(span, a, b) => TypeExpr::Fun(
            *span,
            Box::new(rename_type_expr(a, type_renames, class_renames)),
            Box::new(rename_type_expr(b, type_renames, class_renames)),
        ),
        TypeExpr::Tuple(span, elems) => TypeExpr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| rename_type_expr(e, type_renames, class_renames))
                .collect(),
        ),
        TypeExpr::Record(span, fields) => TypeExpr::Record(
            *span,
            fields
                .iter()
                .map(|(name, ty)| {
                    (
                        name.clone(),
                        rename_type_expr(ty, type_renames, class_renames),
                    )
                })
                .collect(),
        ),
    }
}

fn rename_constraints(
    cs: &[TypeConstraint],
    type_renames: &HashMap<Symbol, Symbol>,
    class_renames: &HashMap<Symbol, Symbol>,
) -> Vec<TypeConstraint> {
    cs.iter()
        .map(|c| TypeConstraint {
            class: class_renames
                .get(&c.class)
                .cloned()
                .unwrap_or_else(|| c.class.clone()),
            typ: rename_type_expr(&c.typ, type_renames, class_renames),
        })
        .collect()
}

fn rename_pattern(pat: &Pattern, value_renames: &HashMap<Symbol, Symbol>) -> Pattern {
    match pat {
        Pattern::Wildcard(span) => Pattern::Wildcard(*span),
        Pattern::Var(v) => Pattern::Var(v.clone()),
        Pattern::Named(span, name, args) => Pattern::Named(
            *span,
            value_renames
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
            args.iter()
                .map(|p| rename_pattern(p, value_renames))
                .collect(),
        ),
        Pattern::Tuple(span, elems) => Pattern::Tuple(
            *span,
            elems
                .iter()
                .map(|p| rename_pattern(p, value_renames))
                .collect(),
        ),
        Pattern::List(span, elems) => Pattern::List(
            *span,
            elems
                .iter()
                .map(|p| rename_pattern(p, value_renames))
                .collect(),
        ),
        Pattern::Cons(span, head, tail) => Pattern::Cons(
            *span,
            Box::new(rename_pattern(head, value_renames)),
            Box::new(rename_pattern(tail, value_renames)),
        ),
        Pattern::Dict(span, fields) => Pattern::Dict(
            *span,
            fields
                .iter()
                .map(|(name, p)| (name.clone(), rename_pattern(p, value_renames)))
                .collect(),
        ),
    }
}

fn rename_expr(
    expr: &Expr,
    bound: &mut HashSet<Symbol>,
    value_renames: &HashMap<Symbol, Symbol>,
    type_renames: &HashMap<Symbol, Symbol>,
    class_renames: &HashMap<Symbol, Symbol>,
) -> Expr {
    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| {
                    Arc::new(rename_expr(
                        e,
                        bound,
                        value_renames,
                        type_renames,
                        class_renames,
                    ))
                })
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| {
                    Arc::new(rename_expr(
                        e,
                        bound,
                        value_renames,
                        type_renames,
                        class_renames,
                    ))
                })
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rename_expr(
                            v,
                            bound,
                            value_renames,
                            type_renames,
                            class_renames,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            Arc::new(rename_expr(
                base,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rename_expr(
                            v,
                            bound,
                            value_renames,
                            type_renames,
                            class_renames,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::Var(v) => {
            if bound.contains(&v.name) {
                Expr::Var(v.clone())
            } else if let Some(new) = value_renames.get(&v.name) {
                Expr::Var(Var {
                    span: v.span,
                    name: new.clone(),
                })
            } else {
                Expr::Var(v.clone())
            }
        }
        Expr::App(span, f, x) => Expr::App(
            *span,
            Arc::new(rename_expr(
                f,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            Arc::new(rename_expr(
                x,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
        ),
        Expr::Project(span, base, field) => Expr::Project(
            *span,
            Arc::new(rename_expr(
                base,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            field.clone(),
        ),
        Expr::Lam(span, scope, param, ann, constraints, body) => {
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                scope.clone(),
                param.clone(),
                ann.as_ref()
                    .map(|t| rename_type_expr(t, type_renames, class_renames)),
                rename_constraints(constraints, type_renames, class_renames),
                Arc::new(rename_expr(
                    body,
                    bound,
                    value_renames,
                    type_renames,
                    class_renames,
                )),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, ann, val, body) => {
            let renamed_val = rename_expr(val, bound, value_renames, type_renames, class_renames);
            bound.insert(var.name.clone());
            let renamed_body = rename_expr(body, bound, value_renames, type_renames, class_renames);
            bound.remove(&var.name);
            Expr::Let(
                *span,
                var.clone(),
                ann.as_ref()
                    .map(|t| rename_type_expr(t, type_renames, class_renames)),
                Arc::new(renamed_val),
                Arc::new(renamed_body),
            )
        }
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            Arc::new(rename_expr(
                c,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            Arc::new(rename_expr(
                t,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            Arc::new(rename_expr(
                e,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
        ),
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = Arc::new(rename_expr(
                scrutinee,
                bound,
                value_renames,
                type_renames,
                class_renames,
            ));
            let mut renamed_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let pat_renamed = rename_pattern(pat, value_renames);
                let mut binds = Vec::new();
                collect_pattern_bindings(&pat_renamed, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = Arc::new(rename_expr(
                    arm_expr,
                    bound,
                    value_renames,
                    type_renames,
                    class_renames,
                ));
                for b in &binds {
                    bound.remove(b);
                }
                renamed_arms.push((pat_renamed, arm_expr));
            }
            Expr::Match(*span, scrutinee, renamed_arms)
        }
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            Arc::new(rename_expr(
                e,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            rename_type_expr(t, type_renames, class_renames),
        ),
    }
}

fn qualify_program(program: &Program, prefix: &str) -> Program {
    let (value_renames, type_renames, class_renames) = collect_local_renames(program, prefix);

    let decls = program
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Import(..) => None,
            Decl::Type(td) => {
                let name = type_renames
                    .get(&td.name)
                    .cloned()
                    .unwrap_or_else(|| td.name.clone());
                let variants = td
                    .variants
                    .iter()
                    .map(|v| rex_ast::expr::TypeVariant {
                        name: value_renames
                            .get(&v.name)
                            .cloned()
                            .unwrap_or_else(|| v.name.clone()),
                        args: v
                            .args
                            .iter()
                            .map(|t| rename_type_expr(t, &type_renames, &class_renames))
                            .collect(),
                    })
                    .collect();
                Some(Decl::Type(TypeDecl {
                    span: td.span,
                    is_pub: td.is_pub,
                    name,
                    params: td.params.clone(),
                    variants,
                }))
            }
            Decl::Fn(fd) => {
                let name_sym = value_renames
                    .get(&fd.name.name)
                    .cloned()
                    .unwrap_or_else(|| fd.name.name.clone());
                let name = Var {
                    span: fd.name.span,
                    name: name_sym,
                };
                let params: Vec<(Var, TypeExpr)> = fd
                    .params
                    .iter()
                    .map(|(v, ann)| {
                        (
                            v.clone(),
                            rename_type_expr(ann, &type_renames, &class_renames),
                        )
                    })
                    .collect();
                let ret = rename_type_expr(&fd.ret, &type_renames, &class_renames);
                let constraints =
                    rename_constraints(&fd.constraints, &type_renames, &class_renames);
                let mut bound = HashSet::new();
                for (v, _) in &params {
                    bound.insert(v.name.clone());
                }
                let body = Arc::new(rename_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    &value_renames,
                    &type_renames,
                    &class_renames,
                ));
                Some(Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name,
                    params,
                    ret,
                    constraints,
                    body,
                }))
            }
            Decl::DeclareFn(df) => {
                let name_sym = value_renames
                    .get(&df.name.name)
                    .cloned()
                    .unwrap_or_else(|| df.name.name.clone());
                let name = Var {
                    span: df.name.span,
                    name: name_sym,
                };
                let params: Vec<(Var, TypeExpr)> = df
                    .params
                    .iter()
                    .map(|(v, ann)| {
                        (
                            v.clone(),
                            rename_type_expr(ann, &type_renames, &class_renames),
                        )
                    })
                    .collect();
                let ret = rename_type_expr(&df.ret, &type_renames, &class_renames);
                let constraints =
                    rename_constraints(&df.constraints, &type_renames, &class_renames);
                Some(Decl::DeclareFn(DeclareFnDecl {
                    span: df.span,
                    is_pub: df.is_pub,
                    name,
                    params,
                    ret,
                    constraints,
                }))
            }
            Decl::Class(cd) => {
                let name = class_renames
                    .get(&cd.name)
                    .cloned()
                    .unwrap_or_else(|| cd.name.clone());
                let supers = rename_constraints(&cd.supers, &type_renames, &class_renames);
                let methods = cd
                    .methods
                    .iter()
                    .map(|m| rex_ast::expr::ClassMethodSig {
                        name: m.name.clone(),
                        typ: rename_type_expr(&m.typ, &type_renames, &class_renames),
                    })
                    .collect();
                Some(Decl::Class(rex_ast::expr::ClassDecl {
                    span: cd.span,
                    is_pub: cd.is_pub,
                    name,
                    params: cd.params.clone(),
                    supers,
                    methods,
                }))
            }
            Decl::Instance(id) => {
                let class = class_renames
                    .get(&id.class)
                    .cloned()
                    .unwrap_or_else(|| id.class.clone());
                let head = rename_type_expr(&id.head, &type_renames, &class_renames);
                let context = rename_constraints(&id.context, &type_renames, &class_renames);
                let mut methods = Vec::new();
                for m in &id.methods {
                    let mut bound = HashSet::new();
                    let body = Arc::new(rename_expr(
                        m.body.as_ref(),
                        &mut bound,
                        &value_renames,
                        &type_renames,
                        &class_renames,
                    ));
                    methods.push(rex_ast::expr::InstanceMethodImpl {
                        name: m.name.clone(),
                        body,
                    });
                }
                Some(Decl::Instance(InstanceDecl {
                    span: id.span,
                    is_pub: id.is_pub,
                    class,
                    head,
                    context,
                    methods,
                }))
            }
        })
        .collect();

    let mut bound = HashSet::new();
    let expr = Arc::new(rename_expr(
        program.expr.as_ref(),
        &mut bound,
        &value_renames,
        &type_renames,
        &class_renames,
    ));

    Program { decls, expr }
}

fn rewrite_import_uses_expr(
    expr: &Expr,
    bound: &mut HashSet<Symbol>,
    aliases: &HashMap<Symbol, ModuleExports>,
) -> Expr {
    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref() {
                if !bound.contains(&v.name) {
                    if let Some(exports) = aliases.get(&v.name) {
                        if let Some(internal) = exports.values.get(field) {
                            return Expr::Var(Var {
                                span: *span,
                                name: internal.clone(),
                            });
                        }
                    }
                }
            }
            Expr::Project(
                *span,
                Arc::new(rewrite_import_uses_expr(base, bound, aliases)),
                field.clone(),
            )
        }
        Expr::Var(v) => Expr::Var(v.clone()),
        Expr::Lam(span, scope, param, ann, constraints, body) => {
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                scope.clone(),
                param.clone(),
                ann.clone(),
                constraints.clone(),
                Arc::new(rewrite_import_uses_expr(body, bound, aliases)),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, ann, val, body) => {
            let val = Arc::new(rewrite_import_uses_expr(val, bound, aliases));
            bound.insert(var.name.clone());
            let body = Arc::new(rewrite_import_uses_expr(body, bound, aliases));
            bound.remove(&var.name);
            Expr::Let(*span, var.clone(), ann.clone(), val, body)
        }
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = Arc::new(rewrite_import_uses_expr(scrutinee, bound, aliases));
            let mut renamed_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = Arc::new(rewrite_import_uses_expr(arm_expr, bound, aliases));
                for b in &binds {
                    bound.remove(b);
                }
                renamed_arms.push((pat.clone(), arm_expr));
            }
            Expr::Match(*span, scrutinee, renamed_arms)
        }
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, aliases)))
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, aliases)))
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, aliases)),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            Arc::new(rewrite_import_uses_expr(base, bound, aliases)),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, aliases)),
                    )
                })
                .collect(),
        ),
        Expr::App(span, f, x) => Expr::App(
            *span,
            Arc::new(rewrite_import_uses_expr(f, bound, aliases)),
            Arc::new(rewrite_import_uses_expr(x, bound, aliases)),
        ),
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            Arc::new(rewrite_import_uses_expr(c, bound, aliases)),
            Arc::new(rewrite_import_uses_expr(t, bound, aliases)),
            Arc::new(rewrite_import_uses_expr(e, bound, aliases)),
        ),
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            Arc::new(rewrite_import_uses_expr(e, bound, aliases)),
            t.clone(),
        ),
    }
}

fn rewrite_import_uses(program: &Program, aliases: &HashMap<Symbol, ModuleExports>) -> Program {
    let decls = program
        .decls
        .iter()
        .map(|decl| match decl {
            Decl::Fn(fd) => {
                let mut bound: HashSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                let body = Arc::new(rewrite_import_uses_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    aliases,
                ));
                Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name: fd.name.clone(),
                    params: fd.params.clone(),
                    ret: fd.ret.clone(),
                    constraints: fd.constraints.clone(),
                    body,
                })
            }
            Decl::Instance(inst) => {
                let methods = inst
                    .methods
                    .iter()
                    .map(|m| {
                        let mut bound = HashSet::new();
                        let body = Arc::new(rewrite_import_uses_expr(
                            m.body.as_ref(),
                            &mut bound,
                            aliases,
                        ));
                        rex_ast::expr::InstanceMethodImpl {
                            name: m.name.clone(),
                            body,
                        }
                    })
                    .collect();
                Decl::Instance(InstanceDecl {
                    span: inst.span,
                    is_pub: inst.is_pub,
                    class: inst.class.clone(),
                    head: inst.head.clone(),
                    context: inst.context.clone(),
                    methods,
                })
            }
            other => other.clone(),
        })
        .collect();

    let mut bound = HashSet::new();
    let expr = Arc::new(rewrite_import_uses_expr(
        program.expr.as_ref(),
        &mut bound,
        aliases,
    ));
    Program { decls, expr }
}

fn validate_import_uses_expr(
    expr: &Expr,
    bound: &mut HashSet<Symbol>,
    aliases: &HashMap<Symbol, ModuleExports>,
) -> Result<(), EngineError> {
    match expr {
        Expr::Project(_, base, field) => {
            if let Expr::Var(v) = base.as_ref() {
                if !bound.contains(&v.name) {
                    if let Some(exports) = aliases.get(&v.name) {
                        if !exports.values.contains_key(field) {
                            return Err(EngineError::Module(format!(
                                "module `{}` does not export `{}`",
                                v.name, field
                            )));
                        }
                    }
                }
            }
            validate_import_uses_expr(base, bound, aliases)
        }
        Expr::Lam(_, _, param, _, _, body) => {
            bound.insert(param.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases);
            bound.remove(&param.name);
            res
        }
        Expr::Let(_, var, _, val, body) => {
            validate_import_uses_expr(val, bound, aliases)?;
            bound.insert(var.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases);
            bound.remove(&var.name);
            res
        }
        Expr::Match(_, scrutinee, arms) => {
            validate_import_uses_expr(scrutinee, bound, aliases)?;
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let res = validate_import_uses_expr(arm_expr, bound, aliases);
                for b in &binds {
                    bound.remove(b);
                }
                res?;
            }
            Ok(())
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for e in elems {
                validate_import_uses_expr(e, bound, aliases)?;
            }
            Ok(())
        }
        Expr::Dict(_, kvs) => {
            for v in kvs.values() {
                validate_import_uses_expr(v, bound, aliases)?;
            }
            Ok(())
        }
        Expr::RecordUpdate(_, base, updates) => {
            validate_import_uses_expr(base, bound, aliases)?;
            for v in updates.values() {
                validate_import_uses_expr(v, bound, aliases)?;
            }
            Ok(())
        }
        Expr::App(_, f, x) => {
            validate_import_uses_expr(f, bound, aliases)?;
            validate_import_uses_expr(x, bound, aliases)
        }
        Expr::Ite(_, c, t, e) => {
            validate_import_uses_expr(c, bound, aliases)?;
            validate_import_uses_expr(t, bound, aliases)?;
            validate_import_uses_expr(e, bound, aliases)
        }
        Expr::Ann(_, e, _) => validate_import_uses_expr(e, bound, aliases),
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..) => Ok(()),
    }
}

fn validate_import_uses(
    program: &Program,
    aliases: &HashMap<Symbol, ModuleExports>,
) -> Result<(), EngineError> {
    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) => {
                let mut bound: HashSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                validate_import_uses_expr(fd.body.as_ref(), &mut bound, aliases)?;
            }
            Decl::Instance(inst) => {
                for m in &inst.methods {
                    let mut bound = HashSet::new();
                    validate_import_uses_expr(m.body.as_ref(), &mut bound, aliases)?;
                }
            }
            _ => {}
        }
    }
    let mut bound = HashSet::new();
    validate_import_uses_expr(program.expr.as_ref(), &mut bound, aliases)
}

fn decl_value_names(decls: &[Decl]) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    for decl in decls {
        match decl {
            Decl::Fn(fd) => {
                out.insert(fd.name.name.clone());
            }
            Decl::DeclareFn(df) => {
                out.insert(df.name.name.clone());
            }
            Decl::Type(td) => {
                for variant in &td.variants {
                    out.insert(variant.name.clone());
                }
            }
            Decl::Class(..) | Decl::Instance(..) | Decl::Import(..) => {}
        }
    }
    out
}

fn rewrite_import_uses_expr_repl(
    expr: &Expr,
    bound: &mut HashSet<Symbol>,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: &HashSet<Symbol>,
) -> Expr {
    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref() {
                if !bound.contains(&v.name) && !shadowed_values.contains(&v.name) {
                    if let Some(exports) = aliases.get(&v.name) {
                        if let Some(internal) = exports.values.get(field) {
                            return Expr::Var(Var {
                                span: *span,
                                name: internal.clone(),
                            });
                        }
                    }
                }
            }
            Expr::Project(
                *span,
                Arc::new(rewrite_import_uses_expr_repl(
                    base,
                    bound,
                    aliases,
                    shadowed_values,
                )),
                field.clone(),
            )
        }
        Expr::Var(v) => Expr::Var(v.clone()),
        Expr::Lam(span, scope, param, ann, constraints, body) => {
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                scope.clone(),
                param.clone(),
                ann.clone(),
                constraints.clone(),
                Arc::new(rewrite_import_uses_expr_repl(
                    body,
                    bound,
                    aliases,
                    shadowed_values,
                )),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, ann, val, body) => {
            let val = Arc::new(rewrite_import_uses_expr_repl(
                val,
                bound,
                aliases,
                shadowed_values,
            ));
            bound.insert(var.name.clone());
            let body = Arc::new(rewrite_import_uses_expr_repl(
                body,
                bound,
                aliases,
                shadowed_values,
            ));
            bound.remove(&var.name);
            Expr::Let(*span, var.clone(), ann.clone(), val, body)
        }
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = Arc::new(rewrite_import_uses_expr_repl(
                scrutinee,
                bound,
                aliases,
                shadowed_values,
            ));
            let mut renamed_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = Arc::new(rewrite_import_uses_expr_repl(
                    arm_expr,
                    bound,
                    aliases,
                    shadowed_values,
                ));
                for b in &binds {
                    bound.remove(b);
                }
                renamed_arms.push((pat.clone(), arm_expr));
            }
            Expr::Match(*span, scrutinee, renamed_arms)
        }
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| {
                    Arc::new(rewrite_import_uses_expr_repl(
                        e,
                        bound,
                        aliases,
                        shadowed_values,
                    ))
                })
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| {
                    Arc::new(rewrite_import_uses_expr_repl(
                        e,
                        bound,
                        aliases,
                        shadowed_values,
                    ))
                })
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr_repl(
                            v,
                            bound,
                            aliases,
                            shadowed_values,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            Arc::new(rewrite_import_uses_expr_repl(
                base,
                bound,
                aliases,
                shadowed_values,
            )),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr_repl(
                            v,
                            bound,
                            aliases,
                            shadowed_values,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::App(span, f, x) => Expr::App(
            *span,
            Arc::new(rewrite_import_uses_expr_repl(
                f,
                bound,
                aliases,
                shadowed_values,
            )),
            Arc::new(rewrite_import_uses_expr_repl(
                x,
                bound,
                aliases,
                shadowed_values,
            )),
        ),
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            Arc::new(rewrite_import_uses_expr_repl(
                c,
                bound,
                aliases,
                shadowed_values,
            )),
            Arc::new(rewrite_import_uses_expr_repl(
                t,
                bound,
                aliases,
                shadowed_values,
            )),
            Arc::new(rewrite_import_uses_expr_repl(
                e,
                bound,
                aliases,
                shadowed_values,
            )),
        ),
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            Arc::new(rewrite_import_uses_expr_repl(
                e,
                bound,
                aliases,
                shadowed_values,
            )),
            t.clone(),
        ),
    }
}

fn rewrite_import_uses_repl(
    program: &Program,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: &HashSet<Symbol>,
) -> Program {
    let decls = program
        .decls
        .iter()
        .map(|decl| match decl {
            Decl::Fn(fd) => {
                let mut bound: HashSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                let body = Arc::new(rewrite_import_uses_expr_repl(
                    fd.body.as_ref(),
                    &mut bound,
                    aliases,
                    shadowed_values,
                ));
                Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name: fd.name.clone(),
                    params: fd.params.clone(),
                    ret: fd.ret.clone(),
                    constraints: fd.constraints.clone(),
                    body,
                })
            }
            Decl::Instance(inst) => {
                let methods = inst
                    .methods
                    .iter()
                    .map(|m| {
                        let mut bound = HashSet::new();
                        let body = Arc::new(rewrite_import_uses_expr_repl(
                            m.body.as_ref(),
                            &mut bound,
                            aliases,
                            shadowed_values,
                        ));
                        rex_ast::expr::InstanceMethodImpl {
                            name: m.name.clone(),
                            body,
                        }
                    })
                    .collect();
                Decl::Instance(InstanceDecl {
                    span: inst.span,
                    is_pub: inst.is_pub,
                    class: inst.class.clone(),
                    head: inst.head.clone(),
                    context: inst.context.clone(),
                    methods,
                })
            }
            other => other.clone(),
        })
        .collect();

    let mut bound = HashSet::new();
    let expr = Arc::new(rewrite_import_uses_expr_repl(
        program.expr.as_ref(),
        &mut bound,
        aliases,
        shadowed_values,
    ));
    Program { decls, expr }
}

fn validate_import_uses_expr_repl(
    expr: &Expr,
    bound: &mut HashSet<Symbol>,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: &HashSet<Symbol>,
) -> Result<(), EngineError> {
    match expr {
        Expr::Project(_, base, field) => {
            if let Expr::Var(v) = base.as_ref() {
                if !bound.contains(&v.name) && !shadowed_values.contains(&v.name) {
                    if let Some(exports) = aliases.get(&v.name) {
                        if !exports.values.contains_key(field) {
                            return Err(EngineError::Module(format!(
                                "module `{}` does not export `{}`",
                                v.name, field
                            )));
                        }
                    }
                }
            }
            validate_import_uses_expr_repl(base, bound, aliases, shadowed_values)
        }
        Expr::Lam(_, _, param, _, _, body) => {
            bound.insert(param.name.clone());
            let res = validate_import_uses_expr_repl(body, bound, aliases, shadowed_values);
            bound.remove(&param.name);
            res
        }
        Expr::Let(_, var, _, val, body) => {
            validate_import_uses_expr_repl(val, bound, aliases, shadowed_values)?;
            bound.insert(var.name.clone());
            let res = validate_import_uses_expr_repl(body, bound, aliases, shadowed_values);
            bound.remove(&var.name);
            res
        }
        Expr::Match(_, scrutinee, arms) => {
            validate_import_uses_expr_repl(scrutinee, bound, aliases, shadowed_values)?;
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let res = validate_import_uses_expr_repl(arm_expr, bound, aliases, shadowed_values);
                for b in &binds {
                    bound.remove(b);
                }
                res?;
            }
            Ok(())
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for e in elems {
                validate_import_uses_expr_repl(e, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::Dict(_, kvs) => {
            for v in kvs.values() {
                validate_import_uses_expr_repl(v, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::RecordUpdate(_, base, updates) => {
            validate_import_uses_expr_repl(base, bound, aliases, shadowed_values)?;
            for v in updates.values() {
                validate_import_uses_expr_repl(v, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::App(_, f, x) => {
            validate_import_uses_expr_repl(f, bound, aliases, shadowed_values)?;
            validate_import_uses_expr_repl(x, bound, aliases, shadowed_values)
        }
        Expr::Ite(_, c, t, e) => {
            validate_import_uses_expr_repl(c, bound, aliases, shadowed_values)?;
            validate_import_uses_expr_repl(t, bound, aliases, shadowed_values)?;
            validate_import_uses_expr_repl(e, bound, aliases, shadowed_values)
        }
        Expr::Ann(_, e, _) => validate_import_uses_expr_repl(e, bound, aliases, shadowed_values),
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..) => Ok(()),
    }
}

fn validate_import_uses_repl(
    program: &Program,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: &HashSet<Symbol>,
) -> Result<(), EngineError> {
    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) => {
                let mut bound: HashSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                validate_import_uses_expr_repl(fd.body.as_ref(), &mut bound, aliases, shadowed_values)?;
            }
            Decl::Instance(inst) => {
                for m in &inst.methods {
                    let mut bound = HashSet::new();
                    validate_import_uses_expr_repl(m.body.as_ref(), &mut bound, aliases, shadowed_values)?;
                }
            }
            _ => {}
        }
    }
    let mut bound = HashSet::new();
    validate_import_uses_expr_repl(program.expr.as_ref(), &mut bound, aliases, shadowed_values)
}

fn exports_from_program(program: &Program, prefix: &str) -> ModuleExports {
    let (value_renames, type_renames, class_renames) = collect_local_renames(program, prefix);

    let mut values = HashMap::new();
    let mut types = HashMap::new();
    let mut classes = HashMap::new();

    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) if fd.is_pub => {
                if let Some(internal) = value_renames.get(&fd.name.name) {
                    values.insert(fd.name.name.clone(), internal.clone());
                }
            }
            Decl::DeclareFn(df) if df.is_pub => {
                if let Some(internal) = value_renames.get(&df.name.name) {
                    values.insert(df.name.name.clone(), internal.clone());
                }
            }
            Decl::Type(td) if td.is_pub => {
                if let Some(internal) = type_renames.get(&td.name) {
                    types.insert(td.name.clone(), internal.clone());
                }
                for variant in &td.variants {
                    if let Some(internal) = value_renames.get(&variant.name) {
                        values.insert(variant.name.clone(), internal.clone());
                    }
                }
            }
            Decl::Class(cd) if cd.is_pub => {
                if let Some(internal) = class_renames.get(&cd.name) {
                    classes.insert(cd.name.clone(), internal.clone());
                }
            }
            Decl::Instance(..)
            | Decl::Import(..)
            | Decl::Fn(..)
            | Decl::DeclareFn(..)
            | Decl::Type(..)
            | Decl::Class(..) => {}
        }
    }

    ModuleExports {
        values,
        types,
        classes,
    }
}

pub fn default_local_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        async move {
            if req.module_name.starts_with("https://") {
                return Ok(None);
            }

            let (module_name, expected_sha) = match req.module_name.split_once('#') {
                Some((a, b)) if !b.is_empty() => (a.to_string(), Some(b.to_string())),
                _ => (req.module_name, None),
            };

            let base_dir = match req.importer {
                Some(ModuleId::Local { path, .. }) => path.parent().map(|p| p.to_path_buf()),
                _ => std::env::current_dir().ok(),
            }
            .ok_or_else(|| {
                EngineError::Module("cannot resolve local import without a base directory".into())
            })?;

            let segs: Vec<&str> = module_name.split('.').collect();
            if segs.is_empty() {
                return Ok(None);
            }

            let mut dir = base_dir;
            let mut idx = 0usize;
            while idx < segs.len() && segs[idx] == "super" {
                dir = dir.parent().map(|p| p.to_path_buf()).ok_or_else(|| {
                    EngineError::Module("import path escapes filesystem root".into())
                })?;
                idx += 1;
            }

            let mut path = dir;
            for seg in &segs[idx..segs.len().saturating_sub(1)] {
                path.push(seg);
            }
            let last = segs
                .last()
                .ok_or_else(|| EngineError::Module("empty module path".into()))?;
            path.push(format!("{last}.rex"));

            let Ok(canon) = path.canonicalize() else {
                return Ok(None);
            };
            let bytes = match std::fs::read(&canon) {
                Ok(b) => b,
                Err(_) => return Ok(None),
            };
            let hash = sha256_hex(&bytes);
            if let Some(expected) = expected_sha {
                let expected = expected.to_ascii_lowercase();
                if !hash.starts_with(&expected) {
                    return Err(EngineError::Module(format!(
                        "local import sha mismatch for {}: expected #{}, got #{hash}",
                        canon.display(),
                        expected
                    )));
                }
            }
            let source = String::from_utf8(bytes)
                .map_err(|e| EngineError::Module(format!("local module was not utf-8: {e}")))?;
            Ok(Some(ResolvedModule {
                id: ModuleId::Local { path: canon, hash },
                source,
            }))
        }
        .boxed()
    })
}

pub fn include_resolver(root: PathBuf) -> ResolverFn {
    Arc::new(move |req: ResolveRequest| {
        let root = root.clone();
        async move {
            if req.module_name.starts_with("https://") {
                return Ok(None);
            }

            let (module_name, expected_sha) = match req.module_name.split_once('#') {
                Some((a, b)) if !b.is_empty() => (a.to_string(), Some(b.to_string())),
                _ => (req.module_name, None),
            };

            let segs: Vec<&str> = module_name.split('.').collect();
            if segs.is_empty() {
                return Ok(None);
            }
            let mut path = root;
            for seg in &segs[..segs.len().saturating_sub(1)] {
                path.push(seg);
            }
            let last = segs
                .last()
                .ok_or_else(|| EngineError::Module("empty module path".into()))?;
            path.push(format!("{last}.rex"));

            let Ok(canon) = path.canonicalize() else {
                return Ok(None);
            };
            let bytes = match std::fs::read(&canon) {
                Ok(b) => b,
                Err(_) => return Ok(None),
            };
            let hash = sha256_hex(&bytes);
            if let Some(expected) = expected_sha {
                let expected = expected.to_ascii_lowercase();
                if !hash.starts_with(&expected) {
                    return Err(EngineError::Module(format!(
                        "include import sha mismatch for {}: expected #{}, got #{hash}",
                        canon.display(),
                        expected
                    )));
                }
            }
            let source = String::from_utf8(bytes)
                .map_err(|e| EngineError::Module(format!("local module was not utf-8: {e}")))?;
            Ok(Some(ResolvedModule {
                id: ModuleId::Local { path: canon, hash },
                source,
            }))
        }
        .boxed()
    })
}

pub fn default_github_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        async move {
        let url = req.module_name;
        let Some(rest) = url.strip_prefix("https://github.com/") else {
            return Ok(None);
        };

        let (path_part, sha_opt) = match rest.split_once('#') {
            Some((a, b)) if !b.is_empty() => (a, Some(b.to_string())),
            _ => (rest, None),
        };

        let mut parts = path_part.splitn(3, '/');
        let owner = parts.next().unwrap_or("");
        let repo = parts.next().unwrap_or("");
        let file_path = parts.next().unwrap_or("");
        if owner.is_empty() || repo.is_empty() || file_path.is_empty() {
            return Err(EngineError::Module(format!(
                "github import must be `https://github.com/<owner>/<repo>/<path>.rex[#sha]` (got {url})"
            )));
        }

        let sha = match sha_opt {
            Some(sha) => sha,
            None => {
                tracing::warn!(
                    "github import `{}` has no #sha; using latest commit on master",
                    url
                );
                let api_url =
                    format!("https://api.github.com/repos/{owner}/{repo}/commits/master");
                let output = Command::new("curl")
                    .arg("-fsSL")
                    .arg("-H")
                    .arg("User-Agent: rex")
                    .arg(&api_url)
                    .output()
                    .map_err(|e| EngineError::Module(format!("failed to run curl: {e}")))?;
                if !output.status.success() {
                    return Err(EngineError::Module(format!(
                        "failed to fetch {api_url} (curl exit {})",
                        output.status
                    )));
                }
                let body = String::from_utf8(output.stdout)
                    .map_err(|e| EngineError::Module(format!("github api response was not utf-8: {e}")))?;
                let needle = "\"sha\":\"";
                let start = body
                    .find(needle)
                    .ok_or_else(|| EngineError::Module("github api response missing sha".into()))?
                    + needle.len();
                let end = body[start..]
                    .find('"')
                    .ok_or_else(|| EngineError::Module("github api response missing sha terminator".into()))?
                    + start;
                body[start..end].to_string()
            }
        };

        let raw_url = format!(
            "https://raw.githubusercontent.com/{owner}/{repo}/{sha}/{file_path}"
        );

        let output = Command::new("curl")
            .arg("-fsSL")
            .arg(&raw_url)
            .output()
            .map_err(|e| EngineError::Module(format!("failed to run curl: {e}")))?;
        if !output.status.success() {
            return Err(EngineError::Module(format!(
                "failed to fetch {raw_url} (curl exit {})",
                output.status
            )));
        }
        let source = String::from_utf8(output.stdout)
            .map_err(|e| EngineError::Module(format!("remote module was not utf-8: {e}")))?;

        let canonical = if url.contains('#') {
            url
        } else {
            format!("{url}#{sha}")
        };

        Ok(Some(ResolvedModule {
            id: ModuleId::Remote(canonical),
            source,
        }))
    }
    .boxed()
    })
}

fn stdlib_source(module: &str) -> Option<&'static str> {
    match module {
        "std.io" => Some(
            r#"
pub declare fn debug (x: a) -> string where Pretty a
pub declare fn info (x: a) -> string where Pretty a
pub declare fn warn (x: a) -> string where Pretty a
pub declare fn error (x: a) -> string where Pretty a

pub declare fn write_all (fd: i32) -> (contents: Array u8) -> ()
pub declare fn read_all (fd: i32) -> Array u8
"#,
        ),
        "std.process" => Some(
            r#"
pub type Subprocess = Subprocess { id: uuid }

pub declare fn spawn (opts: { cmd: string, args: List string }) -> Subprocess
pub declare fn wait (p: Subprocess) -> i32
pub declare fn stdout (p: Subprocess) -> Array u8
pub declare fn stderr (p: Subprocess) -> Array u8
"#,
        ),
        "std.json" => Some(
            r#"
{- JSON values and typeclass-based conversion.

   Note: Rex class method names are global, so we intentionally use
   encode_json/decode_json as the method names and expose
   module-scoped to_json/from_json wrappers.
-}

pub type Value
    = Null
    | Bool bool
    | String string
    | Number f64
    | Array (Array Value)
    | Object (Dict Value)

	pub type DecodeError = DecodeError { message: string }

pub class EncodeJson a where
    encode_json : a -> Value

 	pub class DecodeJson a where
 	    decode_json : Value -> Result a DecodeError

pub fn to_json : a -> Value where EncodeJson a
    = encode_json

 	pub fn from_json : Value -> Result a DecodeError where DecodeJson a
 	    = decode_json

pub fn stringify : Value -> string
    = prim_json_stringify

pub fn parse : string -> Result Value DecodeError
    = (\s ->
        match (prim_json_parse s)
            when Ok v -> Ok v
            when Err msg -> Err (DecodeError { message = msg })
      )

instance Pretty Value
    pretty = stringify

fn fail : string -> Result a DecodeError
    = \msg -> Err (DecodeError { message = msg })

fn kind : Value -> string
    = (\v -> match v
        when Null -> "null"
        when Bool _ -> "bool"
        when String _ -> "string"
        when Number _ -> "number"
        when Array _ -> "array"
        when Object _ -> "object"
      )

fn expected : string -> Value -> DecodeError
    = \want got -> DecodeError { message = "expected " + want + ", got " + kind got }

instance EncodeJson Value
    encode_json = \v -> v

	instance DecodeJson Value
	    decode_json = \v -> Ok v

instance EncodeJson bool
    encode_json = \b -> Bool b

	instance DecodeJson bool
	    decode_json = \v ->
	        match v
	            when Bool b -> Ok b
	            when _ -> Err (expected "bool" v)

instance EncodeJson string
    encode_json = \s -> String s

	instance DecodeJson string
	    decode_json = \v ->
	        match v
	            when String s -> Ok s
	            when _ -> Err (expected "string" v)

instance EncodeJson f64
    encode_json = \n -> Number n

	instance DecodeJson f64
	    decode_json = \v ->
	        match v
	            when Number n -> Ok n
	            when _ -> Err (expected "number" v)

instance EncodeJson f32
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson f32
	    decode_json = \v ->
	        match v
	            when Number n -> (
	                match (prim_f64_to_f32 n)
	                    when Some x -> Ok x
	                    when None -> fail "expected finite f64 representable as f32"
	              )
	            when _ -> Err (expected "number" v)

instance EncodeJson u8
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson u8
	    decode_json = \v ->
	        match v
	            when Number n -> (
	                match (prim_f64_to_u8 n)
	                    when Some x -> Ok x
	                    when None -> fail "expected integer number representable as u8"
	              )
	            when _ -> Err (expected "number" v)

instance EncodeJson u16
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson u16
	    decode_json = \v ->
	        match v
	            when Number n -> (
	                match (prim_f64_to_u16 n)
	                    when Some x -> Ok x
	                    when None -> fail "expected integer number representable as u16"
              )
            when _ -> Err (expected "number" v)

instance EncodeJson u32
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson u32
	    decode_json = \v ->
	        match v
            when Number n -> (
                match (prim_f64_to_u32 n)
                    when Some x -> Ok x
                    when None -> fail "expected integer number representable as u32"
              )
            when _ -> Err (expected "number" v)

instance EncodeJson u64
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson u64
	    decode_json = \v ->
	        match v
            when Number n -> (
                match (prim_f64_to_u64 n)
                    when Some x -> Ok x
                    when None -> fail "expected integer number representable as u64"
              )
            when _ -> Err (expected "number" v)

instance EncodeJson i8
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson i8
	    decode_json = \v ->
	        match v
            when Number n -> (
                match (prim_f64_to_i8 n)
                    when Some x -> Ok x
                    when None -> fail "expected integer number representable as i8"
              )
            when _ -> Err (expected "number" v)

instance EncodeJson i16
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson i16
	    decode_json = \v ->
	        match v
            when Number n -> (
                match (prim_f64_to_i16 n)
                    when Some x -> Ok x
                    when None -> fail "expected integer number representable as i16"
              )
            when _ -> Err (expected "number" v)

instance EncodeJson i32
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson i32
	    decode_json = \v ->
	        match v
            when Number n -> (
                match (prim_f64_to_i32 n)
                    when Some x -> Ok x
                    when None -> fail "expected integer number representable as i32"
              )
            when _ -> Err (expected "number" v)

instance EncodeJson i64
    encode_json = \n -> Number (prim_to_f64 n)

	instance DecodeJson i64
	    decode_json = \v ->
	        match v
            when Number n -> (
                match (prim_f64_to_i64 n)
                    when Some x -> Ok x
                    when None -> fail "expected integer number representable as i64"
              )
            when _ -> Err (expected "number" v)

instance EncodeJson uuid
    encode_json = \u -> String (pretty u)

	instance DecodeJson uuid
	    decode_json = \v ->
	        match v
            when String s -> (
                match (prim_parse_uuid s)
                    when Some u -> Ok u
                    when None -> fail "expected uuid string"
              )
            when _ -> Err (expected "string" v)

instance EncodeJson datetime
    encode_json = \d -> String (pretty d)

	instance DecodeJson datetime
	    decode_json = \v ->
	        match v
            when String s -> (
                match (prim_parse_datetime s)
                    when Some d -> Ok d
                    when None -> fail "expected RFC3339 datetime string"
              )
            when _ -> Err (expected "string" v)

instance EncodeJson (Option a) <= EncodeJson a
    encode_json = \opt ->
        match opt
            when Some x -> to_json x
            when None -> Null

	instance DecodeJson (Option a) <= DecodeJson a
	    decode_json = \v ->
	        match v
	            when Null -> Ok None
	            when _ ->
	                match (from_json v)
	                    when Ok x -> Ok (Some x)
	                    when Err e -> Err e

	instance EncodeJson (Result a e) <= EncodeJson a, EncodeJson e
	    encode_json = \r ->
	        match r
	            when Ok x -> Object { ok = to_json x }
	            when Err e0 -> Object { err = to_json e0 }

	instance DecodeJson (Result a e) <= DecodeJson a, DecodeJson e
	    decode_json = \v ->
	        match v
	            when Object d -> (
	                match d
	                    when {ok, err} -> fail "expected object with exactly one of {ok} or {err}"
	                    when {ok} -> (
	                        match (from_json ok)
	                            when Ok x -> Ok (Ok x)
	                            when Err e -> Err e
	                      )
	                    when {err} -> (
	                        match (from_json err)
	                            when Ok e0 -> Ok (Err e0)
	                            when Err e -> Err e
	                      )
	                    when {} -> fail "expected object with {ok} or {err}"
	              )
	            when _ -> Err (expected "object" v)

instance EncodeJson (List a) <= EncodeJson a
    encode_json = \xs ->
        Array (prim_array_from_list (map (\x -> to_json x) xs))

	instance DecodeJson (List a) <= DecodeJson a
	    decode_json = \v ->
	        match v
	            when Array xs ->
	                let step = \x acc -> match acc
	                        when Err e -> Err e
	                        when Ok out ->
	                            match (from_json x)
	                                when Err e2 -> Err e2
	                                when Ok y -> Ok (Cons y out)
	                in
	                    foldr step (Ok []) xs
	            when _ -> Err (expected "array" v)

instance EncodeJson (Array a) <= EncodeJson a
    encode_json = \xs -> Array (map (\x -> to_json x) xs)

	instance DecodeJson (Array a) <= DecodeJson a
	    decode_json = \v ->
	        match v
	            when Array xs ->
	                let step = \x acc -> match acc
	                        when Err e -> Err e
	                        when Ok out ->
	                            match (from_json x)
	                                when Err e2 -> Err e2
	                                when Ok y -> Ok (Cons y out)
	                in
	                    (
                        match (foldr step (Ok []) xs)
                            when Err e -> Err e
                            when Ok ys -> Ok (prim_array_from_list ys)
                    )
            when _ -> Err (expected "array" v)

instance EncodeJson (Dict a) <= EncodeJson a
    encode_json = \d -> Object (prim_dict_map (\x -> to_json x) d)

	instance DecodeJson (Dict a) <= DecodeJson a
	    decode_json = \v ->
	        match v
	            when Object d -> prim_dict_traverse_result (\x -> from_json x) d
	            when _ -> Err (expected "object" v)
"#,
        ),
        _ => None,
    }
}

pub fn default_stdlib_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        async move {
            let (base, expected_sha) = if let Some((a, b)) = req.module_name.split_once('#') {
                (a, Some(b))
            } else {
                (req.module_name.as_str(), None)
            };

            let Some(source) = stdlib_source(base) else {
                return Ok(None);
            };

            if let Some(expected) = expected_sha {
                let hash = sha256_hex(source.as_bytes());
                let expected = expected.to_ascii_lowercase();
                if !hash.starts_with(&expected) {
                    return Err(EngineError::Module(format!(
                        "sha mismatch for `{base}`: expected #{expected}, got #{hash}"
                    )));
                }
            }

            Ok(Some(ResolvedModule {
                id: ModuleId::Virtual(base.to_string()),
                source: source.to_string(),
            }))
        }
        .boxed()
    })
}

impl Engine {
    pub fn add_resolver<F, Fut>(&mut self, name: impl Into<String>, f: F)
    where
        F: Fn(ResolveRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Option<ResolvedModule>, EngineError>>
            + Send
            + 'static,
    {
        self.modules
            .add_resolver(name, Arc::new(move |req| f(req).boxed()));
    }

    pub fn add_default_resolvers(&mut self) {
        self.modules.add_resolver("stdlib", default_stdlib_resolver());
        self.modules.add_resolver("local", default_local_resolver());
        self.modules
            .add_resolver("remote", default_github_resolver());
    }

    pub fn add_include_resolver(&mut self, root: impl AsRef<Path>) -> Result<(), EngineError> {
        let canon = root
            .as_ref()
            .canonicalize()
            .map_err(|e| EngineError::Module(format!("invalid include root: {e}")))?;
        self.modules.add_resolver(
            format!("include:{}", canon.display()),
            include_resolver(canon),
        );
        Ok(())
    }

    fn load_module_from_resolved(
        &mut self,
        resolved: ResolvedModule,
    ) -> Result<ModuleInstance, EngineError> {
        if let Some(inst) = self.modules.cached(&resolved.id) {
            return Ok(inst);
        }

        self.modules.mark_loading(&resolved.id)?;

        let prefix = prefix_for_module(&resolved.id);
        let tokens = Token::tokenize(&resolved.source).map_err(|e| {
            EngineError::Module(format!("lex error in module {}: {e}", resolved.id))
        })?;
        let mut parser = RexParser::new(tokens);
        let program = parser.parse_program().map_err(|errs| {
            let mut out = format!("parse error in module {}:", resolved.id);
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            EngineError::Module(out)
        })?;

        // Resolve imports first so qualified names exist in the environment.
        let mut alias_exports: HashMap<Symbol, ModuleExports> = HashMap::new();
        for decl in &program.decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: Some(resolved.id.clone()),
            })?;
            let inst = self.load_module_from_resolved(imported)?;
            alias_exports.insert(alias.clone(), inst.exports.clone());
        }

        // Qualify local names, then rewrite `alias.foo` uses into internal symbols.
        let qualified = qualify_program(&program, &prefix);
        validate_import_uses(&qualified, &alias_exports)?;
        let rewritten = rewrite_import_uses(&qualified, &alias_exports);

        self.inject_decls(&rewritten.decls)?;
        let init_value = self.eval(rewritten.expr.as_ref())?;

        let exports = exports_from_program(&program, &prefix);
        let inst = ModuleInstance {
            id: resolved.id.clone(),
            exports,
            init_value,
        };
        self.modules.store_loaded(inst.clone())?;
        Ok(inst)
    }

    fn load_module_from_resolved_with_gas(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
    ) -> Result<ModuleInstance, EngineError> {
        if let Some(inst) = self.modules.cached(&resolved.id) {
            return Ok(inst);
        }

        self.modules.mark_loading(&resolved.id)?;

        let prefix = prefix_for_module(&resolved.id);
        let tokens = Token::tokenize(&resolved.source).map_err(|e| {
            EngineError::Module(format!("lex error in module {}: {e}", resolved.id))
        })?;
        let mut parser = RexParser::new(tokens);
        let program = parser.parse_program_with_gas(gas).map_err(|errs| {
            let mut out = format!("parse error in module {}:", resolved.id);
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            EngineError::Module(out)
        })?;

        let mut alias_exports: HashMap<Symbol, ModuleExports> = HashMap::new();
        for decl in &program.decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: Some(resolved.id.clone()),
            })?;
            let inst = self.load_module_from_resolved_with_gas(imported, gas)?;
            alias_exports.insert(alias.clone(), inst.exports.clone());
        }

        let qualified = qualify_program(&program, &prefix);
        validate_import_uses(&qualified, &alias_exports)?;
        let rewritten = rewrite_import_uses(&qualified, &alias_exports);

        self.inject_decls(&rewritten.decls)?;
        let init_value = self.eval_with_gas(rewritten.expr.as_ref(), gas)?;

        let exports = exports_from_program(&program, &prefix);
        let inst = ModuleInstance {
            id: resolved.id.clone(),
            exports,
            init_value,
        };
        self.modules.store_loaded(inst.clone())?;
        Ok(inst)
    }

    fn load_module_types_from_resolved_with_gas(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
        loaded: &mut HashMap<ModuleId, ModuleExports>,
        loading: &mut HashSet<ModuleId>,
    ) -> Result<ModuleExports, EngineError> {
        if let Some(exports) = loaded.get(&resolved.id) {
            return Ok(exports.clone());
        }

        if loading.contains(&resolved.id) {
            return Err(EngineError::Module(format!(
                "cyclic module import detected at {}",
                resolved.id
            )));
        }
        loading.insert(resolved.id.clone());

        let prefix = prefix_for_module(&resolved.id);
        let tokens = Token::tokenize(&resolved.source).map_err(|e| {
            EngineError::Module(format!("lex error in module {}: {e}", resolved.id))
        })?;
        let mut parser = RexParser::new(tokens);
        let program = parser.parse_program_with_gas(gas).map_err(|errs| {
            let mut out = format!("parse error in module {}:", resolved.id);
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            EngineError::Module(out)
        })?;

        let rewritten = self.rewrite_program_with_imports(
            &program,
            Some(resolved.id.clone()),
            &prefix,
            gas,
            loaded,
            loading,
        )?;
        self.inject_decls(&rewritten.decls)?;

        let exports = exports_from_program(&program, &prefix);
        loaded.insert(resolved.id.clone(), exports.clone());
        loading.remove(&resolved.id);
        Ok(exports)
    }

    fn rewrite_program_with_imports(
        &mut self,
        program: &Program,
        importer: Option<ModuleId>,
        prefix: &str,
        gas: &mut GasMeter,
        loaded: &mut HashMap<ModuleId, ModuleExports>,
        loading: &mut HashSet<ModuleId>,
    ) -> Result<Program, EngineError> {
        let mut alias_exports: HashMap<Symbol, ModuleExports> = HashMap::new();
        for decl in &program.decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: importer.clone(),
            })?;
            let exports =
                self.load_module_types_from_resolved_with_gas(imported, gas, loaded, loading)?;
            alias_exports.insert(alias.clone(), exports);
        }

        let qualified = qualify_program(program, prefix);
        validate_import_uses(&qualified, &alias_exports)?;
        Ok(rewrite_import_uses(&qualified, &alias_exports))
    }

    pub fn infer_module_file_with_gas(
        &mut self,
        path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let canon = path
            .as_ref()
            .canonicalize()
            .map_err(|e| EngineError::Module(format!("invalid module path: {e}")))?;
        let bytes = std::fs::read(&canon)
            .map_err(|e| EngineError::Module(format!("failed to read module: {e}")))?;
        let hash = sha256_hex(&bytes);
        let id = ModuleId::Local {
            path: canon.clone(),
            hash,
        };
        let source = String::from_utf8(bytes)
            .map_err(|e| EngineError::Module(format!("local module was not utf-8: {e}")))?;
        self.infer_module_source_with_gas(ResolvedModule { id, source }, gas)
    }

    fn infer_module_source_with_gas(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let mut loaded: HashMap<ModuleId, ModuleExports> = HashMap::new();
        let mut loading: HashSet<ModuleId> = HashSet::new();

        loading.insert(resolved.id.clone());

        let prefix = prefix_for_module(&resolved.id);
        let tokens = Token::tokenize(&resolved.source).map_err(|e| {
            EngineError::Module(format!("lex error in module {}: {e}", resolved.id))
        })?;
        let mut parser = RexParser::new(tokens);
        let program = parser.parse_program_with_gas(gas).map_err(|errs| {
            let mut out = format!("parse error in module {}:", resolved.id);
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            EngineError::Module(out)
        })?;

        let rewritten = self.rewrite_program_with_imports(
            &program,
            Some(resolved.id.clone()),
            &prefix,
            gas,
            &mut loaded,
            &mut loading,
        )?;
        self.inject_decls(&rewritten.decls)?;

        let (preds, ty) = self.infer_type_with_gas(rewritten.expr.as_ref(), gas)?;

        let exports = exports_from_program(&program, &prefix);
        loaded.insert(resolved.id.clone(), exports);
        loading.remove(&resolved.id);

        Ok((preds, ty))
    }

    pub fn infer_snippet_with_gas(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        self.infer_snippet_with_gas_and_importer(source, gas, None)
    }

    pub fn infer_snippet_at_with_gas(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let path = importer_path.as_ref().to_path_buf();
        self.infer_snippet_with_gas_and_importer(source, gas, Some(path))
    }

    fn infer_snippet_with_gas_and_importer(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
        importer_path: Option<PathBuf>,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let tokens =
            Token::tokenize(source).map_err(|e| EngineError::Module(format!("lex error: {e}")))?;
        let mut parser = RexParser::new(tokens);
        let program = parser.parse_program_with_gas(gas).map_err(|errs| {
            let mut out = String::from("parse error:");
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            EngineError::Module(out)
        })?;

        let importer = importer_path.map(|p| ModuleId::Local {
            path: p,
            hash: "snippet".into(),
        });

        let mut loaded: HashMap<ModuleId, ModuleExports> = HashMap::new();
        let mut loading: HashSet<ModuleId> = HashSet::new();

        let prefix = format!("@snippet{}", Uuid::new_v4());
        let rewritten = self.rewrite_program_with_imports(
            &program,
            importer,
            &prefix,
            gas,
            &mut loaded,
            &mut loading,
        )?;
        self.inject_decls(&rewritten.decls)?;
        self.infer_type_with_gas(rewritten.expr.as_ref(), gas)
    }

    pub fn eval_module_file(&mut self, path: impl AsRef<Path>) -> Result<Value, EngineError> {
        let canon = path
            .as_ref()
            .canonicalize()
            .map_err(|e| EngineError::Module(format!("invalid module path: {e}")))?;
        let bytes = std::fs::read(&canon)
            .map_err(|e| EngineError::Module(format!("failed to read module: {e}")))?;
        let hash = sha256_hex(&bytes);
        let id = ModuleId::Local {
            path: canon.clone(),
            hash,
        };
        if let Some(inst) = self.modules.cached(&id) {
            return Ok(inst.init_value);
        }
        let source = String::from_utf8(bytes)
            .map_err(|e| EngineError::Module(format!("local module was not utf-8: {e}")))?;
        let inst = self.load_module_from_resolved(ResolvedModule { id, source })?;
        Ok(inst.init_value)
    }

    pub fn eval_module_file_with_gas(
        &mut self,
        path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        let canon = path
            .as_ref()
            .canonicalize()
            .map_err(|e| EngineError::Module(format!("invalid module path: {e}")))?;
        let bytes = std::fs::read(&canon)
            .map_err(|e| EngineError::Module(format!("failed to read module: {e}")))?;
        let hash = sha256_hex(&bytes);
        let id = ModuleId::Local {
            path: canon.clone(),
            hash,
        };
        if let Some(inst) = self.modules.cached(&id) {
            return Ok(inst.init_value);
        }
        let source = String::from_utf8(bytes)
            .map_err(|e| EngineError::Module(format!("local module was not utf-8: {e}")))?;
        let inst = self.load_module_from_resolved_with_gas(ResolvedModule { id, source }, gas)?;
        Ok(inst.init_value)
    }

    pub fn eval_module_source(&mut self, source: &str) -> Result<Value, EngineError> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let id = ModuleId::Virtual(format!("<inline:{:016x}>", hasher.finish()));
        if let Some(inst) = self.modules.cached(&id) {
            return Ok(inst.init_value);
        }
        let inst = self.load_module_from_resolved(ResolvedModule {
            id,
            source: source.to_string(),
        })?;
        Ok(inst.init_value)
    }

    pub fn eval_module_source_with_gas(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let id = ModuleId::Virtual(format!("<inline:{:016x}>", hasher.finish()));
        if let Some(inst) = self.modules.cached(&id) {
            return Ok(inst.init_value);
        }
        let inst = self.load_module_from_resolved_with_gas(
            ResolvedModule {
                id,
                source: source.to_string(),
            },
            gas,
        )?;
        Ok(inst.init_value)
    }

    pub fn eval_snippet(&mut self, source: &str) -> Result<Value, EngineError> {
        self.eval_snippet_with_importer(source, None)
    }

    pub fn eval_snippet_with_gas(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        self.eval_snippet_with_gas_and_importer(source, gas, None)
    }

    pub fn eval_repl_program_with_gas(
        &mut self,
        program: &Program,
        state: &mut ReplState,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        let importer = state.importer_path.as_ref().map(|p| ModuleId::Local {
            path: p.clone(),
            hash: "repl".into(),
        });

        for decl in &program.decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: importer.clone(),
            })?;
            let inst = self.load_module_from_resolved_with_gas(imported, gas)?;
            state.alias_exports.insert(alias.clone(), inst.exports.clone());
        }

        let mut shadowed_values = state.defined_values.clone();
        shadowed_values.extend(decl_value_names(&program.decls));

        validate_import_uses_repl(program, &state.alias_exports, &shadowed_values)?;
        let rewritten = rewrite_import_uses_repl(program, &state.alias_exports, &shadowed_values);

        self.inject_decls(&rewritten.decls)?;
        state.defined_values.extend(decl_value_names(&program.decls));
        self.eval_with_gas(rewritten.expr.as_ref(), gas)
    }

    /// Evaluate a non-module snippet, but use `importer_path` for resolving local-relative imports.
    pub fn eval_snippet_at(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
    ) -> Result<Value, EngineError> {
        let path = importer_path.as_ref().to_path_buf();
        self.eval_snippet_with_importer(source, Some(path))
    }

    /// Evaluate a non-module snippet (with gas), but use `importer_path` for resolving local-relative imports.
    pub fn eval_snippet_at_with_gas(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        let path = importer_path.as_ref().to_path_buf();
        self.eval_snippet_with_gas_and_importer(source, gas, Some(path))
    }

    fn eval_snippet_with_importer(
        &mut self,
        source: &str,
        importer_path: Option<PathBuf>,
    ) -> Result<Value, EngineError> {
        let tokens =
            Token::tokenize(source).map_err(|e| EngineError::Module(format!("lex error: {e}")))?;
        let mut parser = RexParser::new(tokens);
        let program = parser.parse_program().map_err(|errs| {
            let mut out = String::from("parse error:");
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            EngineError::Module(out)
        })?;

        let importer = importer_path.map(|p| ModuleId::Local {
            path: p,
            hash: "snippet".into(),
        });

        let mut alias_exports: HashMap<Symbol, ModuleExports> = HashMap::new();
        for decl in &program.decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: importer.clone(),
            })?;
            let inst = self.load_module_from_resolved(imported)?;
            alias_exports.insert(alias.clone(), inst.exports.clone());
        }

        let prefix = format!("@snippet{}", Uuid::new_v4());
        let qualified = qualify_program(&program, &prefix);
        validate_import_uses(&qualified, &alias_exports)?;
        let rewritten = rewrite_import_uses(&qualified, &alias_exports);

        self.inject_decls(&rewritten.decls)?;
        self.eval(rewritten.expr.as_ref())
    }

    fn eval_snippet_with_gas_and_importer(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
        importer_path: Option<PathBuf>,
    ) -> Result<Value, EngineError> {
        let tokens =
            Token::tokenize(source).map_err(|e| EngineError::Module(format!("lex error: {e}")))?;
        let mut parser = RexParser::new(tokens);
        let program = parser.parse_program_with_gas(gas).map_err(|errs| {
            let mut out = String::from("parse error:");
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            EngineError::Module(out)
        })?;

        let importer = importer_path.map(|p| ModuleId::Local {
            path: p,
            hash: "snippet".into(),
        });

        let mut alias_exports: HashMap<Symbol, ModuleExports> = HashMap::new();
        for decl in &program.decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: importer.clone(),
            })?;
            let inst = self.load_module_from_resolved_with_gas(imported, gas)?;
            alias_exports.insert(alias.clone(), inst.exports.clone());
        }

        let prefix = format!("@snippet{}", Uuid::new_v4());
        let qualified = qualify_program(&program, &prefix);
        validate_import_uses(&qualified, &alias_exports)?;
        let rewritten = rewrite_import_uses(&qualified, &alias_exports);

        self.inject_decls(&rewritten.decls)?;
        self.eval_with_gas(rewritten.expr.as_ref(), gas)
    }
}
