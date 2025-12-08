use futures::{stream, StreamExt, TryStreamExt};
use rex_type_system::types::{Dispatch, ToType, Type, TypeEnv, TypeScheme, TypeVar, ADT};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    future::Future,
    mem,
    num::NonZeroUsize,
    pin::Pin,
    str::FromStr,
    sync::Arc,
};

use crate::{
    codec::{Decode, Encode, Func},
    error::{Error, OverlappingFunctions},
    eval::{apply, Context},
    ftable::{Ftable, FtableFn, Namespace, A, B, C, E, F},
};
use chrono::{DateTime, Utc};
use regex::Regex;
use rex_ast::expr::Expr;
use rex_lexer::span::Span;
use uuid::Uuid;

const MAX_TUPLE_LEN: usize = 16; // register elem_M_N functions for tuple sizes up to M

macro_rules! ftable_fn {
    ($f:ident $(, $($param:ident),*)?) => {{
        TypedFunction {
            t: Arc::new(<fn($($($param,)*)?) -> B as ToType>::to_type()),
            f: Box::new(move |ctx, args| {
                let f = ($f).clone();
                Box::pin(async move {
                    let mut i = 0;
                    let mut r = f(ctx $(, $(decode_arg::<$param>(args, { let j = i; i += 1; j })?),*)?)?
                        .try_encode(Span::default())?; // FIXME(loong): assign a proper span
                    while i < args.len() {
                        r = $crate::eval::apply(ctx, r, &args[{ let j = i; i += 1; j }], None).await?;
                    }
                    Ok(r)
                })
            }),
        }
    }}
}

macro_rules! ftable_async_fn {
    ($f:ident $(, $($param:ident),*)?) => {{
        TypedFunction {
            t: Arc::new(<fn($($($param,)*)?) -> B as ToType>::to_type()),
            f: Box::new(move |ctx, args| {
                let f = ($f).clone();
                Box::pin(async move {
                    let mut i = 0;
                    let mut r = f(ctx $(, $(decode_arg::<$param>(args, { let j = i; i += 1; j })?),*)?)
                        .await?
                        .try_encode(Span::default())?; // FIXME(loong): assign a proper span
                    while i < args.len() {
                        r = $crate::eval::apply(ctx, r, &args[{ let j = i; i += 1; j }], None).await?;
                    }
                    Ok(r)
                })
            }),
        }
    }}
}

fn decode_arg<A>(args: &[Arc<Expr>], i: usize) -> Result<A, Error>
where
    A: Decode,
{
    args.get(i)
        .ok_or(Error::MissingArgument {
            argument: i,
            trace: Default::default(),
        })
        .and_then(|a0| A::try_decode(a0))
}

fn register_fn_core<State>(
    builder: &mut Builder<State>,
    ns: &Namespace,
    n: &str,
    t: Arc<Type>,
) -> Result<(), Error>
where
    State: Clone + Send + Sync + 'static,
{
    let unresolved_vars = t.unresolved_vars();

    let scheme = if !unresolved_vars.is_empty() {
        let mut assignments = HashMap::new();
        for var in &unresolved_vars {
            if !assignments.contains_key(var) {
                assignments.insert(var.clone(), Arc::new(Type::Var(TypeVar::new())));
            }
        }

        let t = t.resolve_vars(&assignments);

        let mut vars: Vec<TypeVar> = Vec::new();
        for var in assignments.into_values() {
            if let Type::Var(var) = *var {
                vars.push(var);
            } else {
                panic!("Expected a type variable");
            }
        }
        TypeScheme::new(vars, t)
    } else {
        TypeScheme::from(t)
    };

    match builder.builtins.get_mut(n) {
        None => {
            let mut entry = BuilderEntry::default();
            entry.add(ns, n, scheme)?;
            builder.builtins.insert(n.to_string(), entry);
        }
        Some(entry) => {
            entry.add(ns, n, scheme)?;
        }
    }

    Ok(())
}

pub struct BuilderEntryItem {
    pub ns: Namespace,
    pub name: String,
}

#[derive(Default)]
pub struct BuilderEntry {
    items_by_type_scheme: HashMap<TypeScheme, BuilderEntryItem>,
    category: Option<String>,
    description: Option<String>,
}

impl BuilderEntry {
    pub fn add(&mut self, ns: &Namespace, n: &str, scheme: TypeScheme) -> Result<(), Error> {
        for (s, item) in self.items_by_type_scheme.iter() {
            if scheme.maybe_overlaps_with(s) {
                return Err(Error::OverlappingFunctions {
                    overlap: Arc::new(OverlappingFunctions {
                        name: n.to_string(),
                        t1: scheme.clone(),
                        t2: s.clone(),
                        name1: format!("{}::{}", item.ns, item.name),
                        name2: format!("{}::{}", ns, n),
                    }),
                    trace: Default::default(),
                });
            }
        }

        self.items_by_type_scheme.insert(
            scheme,
            BuilderEntryItem {
                ns: ns.clone(),
                name: n.to_string(),
            },
        );
        Ok(())
    }

    pub fn into_type_schemes(self) -> HashSet<TypeScheme> {
        HashSet::from_iter(self.items_by_type_scheme.into_keys())
    }
}

macro_rules! impl_fn {
    ($name:ident $(,$($param:ident),*)?) => {
        pub fn $name <$($($param,)*)? B, F, State>(
            f: F,
        ) -> TypedFunction<State>
        where
            $($($param : Decode + Send + ToType,)*)?
            State: Clone + Sync + Send + 'static,
            B: Encode + ToType,
            F: Fn(
                &Context<State>,
                $($($param,)*)?
            ) -> Result<B, Error>
                + Clone
                + Send
                + Sync
                + 'static
        {
            ftable_fn!(f, $($($param),*)?)
        }
    };
}

macro_rules! impl_fn_async {
    ($name:ident $(,$($param:ident),*)?) => {
        pub fn $name <$($($param,)*)? B, F, State>(
            f: F,
        ) -> TypedFunction<State>
        where
            $($($param : Decode + Send + ToType,)*)?
            State: Clone + Sync + Send + 'static,
            B: Encode + ToType,
            for<'c> F: Fn(
                &'c Context<State>,
                $($($param,)*)?
            ) -> Pin<Box<dyn Future<Output = Result<B, Error>> + Send + 'c>>
                + Clone
                + Send
                + Sync
                + 'static,
        {
            ftable_async_fn!(f, $($($param),*)?)
        }
    };
}

impl_fn!(fn0);
impl_fn!(fn1, A0);
impl_fn!(fn2, A0, A1);
impl_fn!(fn3, A0, A1, A2);
impl_fn!(fn4, A0, A1, A2, A3);

impl_fn_async!(fn_async1, A0);
impl_fn_async!(fn_async2, A0, A1);
impl_fn_async!(fn_async3, A0, A1, A2);
impl_fn_async!(fn_async4, A0, A1, A2, A3);

pub struct TypedFunction<State>
where
    State: Clone + Sync + 'static,
{
    pub t: Arc<Type>,
    pub f: FtableFn<State>,
}

pub struct Builder<State>
where
    State: Clone + Sync + 'static,
{
    pub builtins: BTreeMap<String, BuilderEntry>,
    pub ftable: Ftable<State>,
    pub adts: BTreeMap<String, ADT>,
    pub accessors: HashSet<String>,
    pub equality_registered: HashSet<String>,
}

pub struct Docs {
    pub categories: BTreeMap<String, CategoryDocs>,
}

impl Docs {
    pub fn to_markdown(&self) -> String {
        let mut output = String::new();
        output.push_str("# Rex functions\n\n");

        for (cat_name, cat_docs) in self.categories.iter() {
            if cat_name.is_empty() {
                continue;
            }

            output.push_str(&format!("# {}\n\n", cat_name));
            cat_docs.to_markdown(&mut output);
        }

        if let Some(cat_docs) = self.categories.get("") {
            output.push_str("# Other\n\n");
            cat_docs.to_markdown(&mut output);
        }

        output
    }
}

pub struct CategoryDocs {
    pub description: Option<String>,
    pub functions: BTreeMap<String, FunctionDocs>,
}

impl CategoryDocs {
    pub fn to_markdown(&self, output: &mut String) {
        if let Some(description) = &self.description {
            output.push_str(description);
            output.push_str("\n\n");
        }
        for (f_name, f_docs) in self.functions.iter() {
            output.push_str(&format!("## `{}`\n\n", f_name));
            f_docs.to_markdown(output);
        }
    }
}

pub struct FunctionDocs {
    pub description: Option<String>,
    pub type_signatures: Vec<String>,
}

impl FunctionDocs {
    pub fn to_markdown(&self, output: &mut String) {
        if self.type_signatures.len() == 1 {
            output.push_str(&format!("Type signature: `{}`\n", self.type_signatures[0]));
        } else {
            output.push_str("Type signatures:\n\n");
            for ts in self.type_signatures.iter() {
                output.push_str(&format!("- `{}`\n", ts));
            }
        }
        output.push('\n');

        if let Some(description) = &self.description {
            output.push_str(description);
            output.push_str("\n\n");
        }
    }
}

impl<State> Builder<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub fn docs(&self) -> Docs {
        let mut by_category: BTreeMap<String, CategoryDocs> = BTreeMap::new();

        for (name, entry) in self.builtins.iter() {
            let entry_category: String = entry
                .category
                .as_ref()
                .map(|x| x.to_string())
                .unwrap_or("".to_string());
            let entry_description: Option<String> =
                entry.description.as_ref().map(|x| x.to_string());
            let cat: &mut CategoryDocs =
                by_category
                    .entry(entry_category.clone())
                    .or_insert_with(|| CategoryDocs {
                        description: None,
                        functions: BTreeMap::new(),
                    });

            let mut type_signatures: Vec<String> = Vec::new();
            for ts in entry.items_by_type_scheme.keys() {
                type_signatures.push(ts.ty.to_string());
            }

            cat.functions.insert(
                name.clone(),
                FunctionDocs {
                    description: entry_description,
                    type_signatures,
                },
            );
        }

        Docs {
            categories: by_category,
        }
    }

    fn set_docs(&mut self, _ns: &Namespace, name: &str, category: &str, description: &str) {
        let Some(entry) = self.builtins.get_mut(name) else {
            panic!("No entry for {}", name);
        };

        entry.category = Some(category.to_string());
        entry.description = Some(description.to_string());
    }

    pub fn with_prelude() -> Result<Self, Error> {
        let mut this = Self {
            builtins: BTreeMap::new(),
            ftable: Ftable::new(),
            adts: Default::default(),
            accessors: Default::default(),
            equality_registered: Default::default(),
        };

        let ns = &Namespace::rex();

        this.register(ns, "swap", fn1(|_, (x, y): (A, B)| Ok((y, x))));

        this.register(ns, "string", fn1(|_, x: bool| Ok(format!("{}", x))));
        this.register(ns, "string", fn1(|_, x: u64| Ok(format!("{}", x))));
        this.register(ns, "string", fn1(|_, x: i64| Ok(format!("{}", x))));
        this.register(ns, "string", fn1(|_, x: f64| Ok(format!("{}", x))));
        this.set_docs(
            ns,
            "string",
            "Type conversion",
            "Convert a value to a string",
        );

        this.register(ns, "uint", fn1(|_, x: i64| Ok(x as u64)));
        this.register(ns, "uint", fn1(|_, x: f64| Ok(x as u64)));
        this.register(ns, "uint", fn1(|_, x: String| Ok(x.parse::<u64>()?)));
        this.set_docs(
            ns,
            "uint",
            "Type conversion",
            "Convert a value to an unsigned integer",
        );

        this.register(ns, "int", fn1(|_, x: u64| Ok(x as i64)));
        this.register(ns, "int", fn1(|_, x: f64| Ok(x as i64)));
        this.register(ns, "int", fn1(|_, x: String| Ok(x.parse::<i64>()?)));
        this.set_docs(
            ns,
            "int",
            "Type conversion",
            "Convert a value to a signed integer",
        );

        this.register(ns, "float", fn1(|_, x: u64| Ok(x as f64)));
        this.register(ns, "float", fn1(|_, x: i64| Ok(x as f64)));
        this.register(ns, "float", fn1(|_, x: String| Ok(x.parse::<f64>()?)));
        this.set_docs(
            ns,
            "float",
            "Type conversion",
            "Convert a value to a double-precision float",
        );

        this.register(ns, "negate", fn1(|_, x: u64| Ok(-(x as i64))));
        this.register(ns, "negate", fn1(|_, x: i64| Ok(-x)));
        this.register(ns, "negate", fn1(|_, x: f64| Ok(-x)));

        this.register(ns, "&&", fn2(|_, x: bool, y: bool| Ok(x && y)));
        this.set_docs(ns, "&&", "Boolean operators", "The **and** operator");
        this.register(ns, "||", fn2(|_, x: bool, y: bool| Ok(x || y)));
        this.set_docs(ns, "||", "Boolean operators", "The **or** operator");

        this.register(ns, "==", fn2(|_, x: u64, y: u64| Ok(x == y)));
        this.register(ns, "==", fn2(|_, x: i64, y: i64| Ok(x == y)));
        this.register(ns, "==", fn2(|_, x: f64, y: f64| Ok(x == y)));
        this.register(ns, "==", fn2(|_, x: String, y: String| Ok(x == y)));

        this.register(ns, "!=", fn2(|_, x: u64, y: u64| Ok(x != y)));
        this.register(ns, "!=", fn2(|_, x: i64, y: i64| Ok(x != y)));
        this.register(ns, "!=", fn2(|_, x: f64, y: f64| Ok(x != y)));
        this.register(ns, "!=", fn2(|_, x: String, y: String| Ok(x != y)));

        this.register(ns, ">", fn2(|_, x: u64, y: u64| Ok(x > y)));
        this.register(ns, ">", fn2(|_, x: i64, y: i64| Ok(x > y)));
        this.register(ns, ">", fn2(|_, x: f64, y: f64| Ok(x > y)));
        this.register(ns, ">", fn2(|_, x: String, y: String| Ok(x > y)));

        this.register(ns, ">=", fn2(|_, x: u64, y: u64| Ok(x >= y)));
        this.register(ns, ">=", fn2(|_, x: i64, y: i64| Ok(x >= y)));
        this.register(ns, ">=", fn2(|_, x: f64, y: f64| Ok(x >= y)));
        this.register(ns, ">=", fn2(|_, x: String, y: String| Ok(x >= y)));

        this.register(ns, "<", fn2(|_, x: u64, y: u64| Ok(x < y)));
        this.register(ns, "<", fn2(|_, x: i64, y: i64| Ok(x < y)));
        this.register(ns, "<", fn2(|_, x: f64, y: f64| Ok(x < y)));
        this.register(ns, "<", fn2(|_, x: String, y: String| Ok(x < y)));

        this.register(ns, "<=", fn2(|_, x: u64, y: u64| Ok(x <= y)));
        this.register(ns, "<=", fn2(|_, x: i64, y: i64| Ok(x <= y)));
        this.register(ns, "<=", fn2(|_, x: f64, y: f64| Ok(x <= y)));
        this.register(ns, "<=", fn2(|_, x: String, y: String| Ok(x <= y)));

        this.register(ns, "+", fn2(|_, x: u64, y: u64| Ok(x + y)));
        this.register(ns, "+", fn2(|_, x: i64, y: i64| Ok(x + y)));
        this.register(ns, "+", fn2(|_, x: f64, y: f64| Ok(x + y)));

        this.register(ns, "-", fn2(|_, x: u64, y: u64| Ok(x - y)));
        this.register(ns, "-", fn2(|_, x: i64, y: i64| Ok(x - y)));
        this.register(ns, "-", fn2(|_, x: f64, y: f64| Ok(x - y)));

        this.register(ns, "*", fn2(|_, x: u64, y: u64| Ok(x * y)));
        this.register(ns, "*", fn2(|_, x: i64, y: i64| Ok(x * y)));
        this.register(ns, "*", fn2(|_, x: f64, y: f64| Ok(x * y)));

        this.register(ns, "/", fn2(|_, x: u64, y: u64| Ok(x / y)));
        this.register(ns, "/", fn2(|_, x: i64, y: i64| Ok(x / y)));
        this.register(ns, "/", fn2(|_, x: f64, y: f64| Ok(x / y)));

        this.register(ns, "%", fn2(|_, x: u64, y: u64| Ok(x % y)));
        this.register(ns, "%", fn2(|_, x: i64, y: i64| Ok(x % y)));
        this.register(ns, "%", fn2(|_, x: f64, y: f64| Ok(x % y)));

        this.register(ns, "abs", fn1(|_, x: i64| Ok(x.abs())));
        this.register(ns, "abs", fn1(|_, x: f64| Ok(x.abs())));

        this.register(ns, "sqrt", fn1(|_, x: f64| Ok(x.sqrt())));

        this.register(ns, "pow", fn2(|_, x: f64, y: i32| Ok(x.powi(y))));
        this.register(ns, "pow", fn2(|_, x: f64, y: f64| Ok(x.powf(y))));

        this.register(ns, "identity", fn1(|_, x: A| Ok(x)));

        this.register(
            ns,
            "get",
            fn2(|_, n: u64, xs: Vec<A>| {
                xs.get(n as usize)
                    .ok_or(Error::Custom {
                        error: "Index out of bounds".to_string(),
                        trace: Default::default(),
                    })
                    .cloned()
            }),
        );

        this.register_elem_functions(ns);

        this.register(
            ns,
            "++",
            fn2(|_, xs: Vec<A>, ys: Vec<A>| {
                let mut zs = Vec::with_capacity(xs.len() + ys.len());
                zs.extend(xs);
                zs.extend(ys);
                Ok(zs)
            }),
        );

        this.register(
            ns,
            "++",
            fn2(|_, a: String, b: String| Ok(format!("{}{}", a, b))),
        );

        this.register(ns, "len", fn1(|_ctx, x: String| Ok(x.len() as u64)));
        this.register(ns, "len", fn1(|_ctx, x: Vec<A>| Ok(x.len() as u64)));

        this.register(
            ns,
            "take",
            fn2(|_, n: u64, xs: Vec<A>| Ok(xs.into_iter().take(n as usize).collect::<Vec<_>>())),
        );

        // Registers function with two parameters to match a regex pattern,
        // Returns True if there is a match, orelse False
        // pattern : Pattern to convert into regex (re),
        // hay : to match against the re-created from pattern

        // Possibly a costly computation while looping, since fn essentially recompiles pattern multiple times (no cache)
        this.register(
            ns,
            "regex_matches",
            fn2(|_, pattern: String, hay: String| {
                match Regex::new(&pattern).map_err(Error::from) {
                    Ok(re) => Ok(re.is_match(&hay)),
                    Err(err) => Err(err), // Return Err(Error)
                }
            }),
        );

        // Registers function with two parameters to return successive non-overlapping matches in given haystack
        // pattern : Pattern to convert into regex (re),
        // hay : to match against the re-created from pattern

        // Possibly a costly computation while looping, since fn essentially recompiles pattern multiple times (no cache)
        this.register(
            ns,
            "regex_captures",
            fn2(|_, pattern: String, hay: String| {
                match Regex::new(&pattern).map_err(Error::from) {
                    Ok(re) => {
                        let matches: Vec<_> =
                            re.find_iter(&hay).map(|m| m.as_str().to_string()).collect();
                        Ok(matches)
                    }
                    Err(err) => Err(err),
                }
            }),
        );

        this.register(
            ns,
            "skip",
            fn2(|_, n: u64, xs: Vec<A>| Ok(xs.into_iter().take(n as usize).collect::<Vec<_>>())),
        );

        this.register(
            ns,
            "zip",
            fn2(|_, xs: Vec<A>, ys: Vec<B>| Ok(xs.into_iter().zip(ys).collect::<Vec<_>>())),
        );

        this.register(
            ns,
            "unzip",
            fn1(|_, zs: Vec<(A, B)>| {
                let mut xs = Vec::with_capacity(zs.len());
                let mut ys = Vec::with_capacity(zs.len());
                for (x, y) in zs {
                    xs.push(x);
                    ys.push(y);
                }
                Ok((xs, ys))
            }),
        );

        this.register(
            ns,
            "flip",
            fn_async3(|ctx, f: Func<A, Func<B, C>>, x: B, y: A| {
                Box::pin(async move {
                    let g = apply(ctx, &f, &y, None).await?;
                    let z = apply(ctx, &g, &x, None).await?;
                    Ok(C(z))
                })
            }),
        );

        this.register(
            ns,
            "map",
            fn_async2(|ctx, f: Func<A, B>, xs: Vec<A>| {
                Box::pin(async move {
                    let mut ys = Vec::with_capacity(xs.len());
                    for x in xs {
                        ys.push(apply(ctx, f.clone(), x.clone(), None));
                    }
                    let nthreads = std::thread::available_parallelism()
                        .unwrap_or(NonZeroUsize::new(1).unwrap())
                        .get()
                        * 2;
                    let exprs: Vec<Arc<Expr>> =
                        stream::iter(ys).buffered(nthreads).try_collect().await?;
                    Ok(exprs.into_iter().map(B).collect::<Vec<B>>())
                })
            }),
        );
        this.set_docs(
            ns,
            "map",
            "Lists",
            concat!(
                "Applies a function to every value in a list, returning a new list",
                "containing the results of each function call\n\n",
                "Example:\n\n",
                "```\n",
                "map (\\x -> x * 2) [3, 4, 5]\n",
                "```\n\n",
                "Returns the value: `[6, 8, 10]`\n\n",
                "`map` can also be used with `Result` and `Option` types. If used on a ",
                "`Result`, the function is only called if the value is of the `Ok` variant. ",
                "If used on an `Option`, the function is only called it the value is of the ",
                "`Some` variant",
            ),
        );

        this.register(
            ns,
            "filter",
            fn_async2(|ctx, f: Func<A, bool>, xs: Vec<A>| {
                Box::pin(async move {
                    let mut ys: Vec<A> = Vec::with_capacity(xs.len());
                    for x in xs {
                        let b = bool::try_decode(&apply(ctx, &f, &x, None).await?)?;
                        if b {
                            ys.push(x);
                        }
                    }
                    Ok(ys)
                })
            }),
        );

        this.register(
            ns,
            "filter_map",
            fn_async2(|ctx, f: Func<A, Option<B>>, xs: Vec<A>| {
                Box::pin(async move {
                    let mut ys: Vec<B> = Vec::with_capacity(xs.len());
                    for x in xs {
                        let y = <Option<B>>::try_decode(&apply(ctx, &f, &x, None).await?)?;
                        if let Some(y) = y {
                            ys.push(y);
                        }
                    }
                    Ok(ys)
                })
            }),
        );

        this.register(
            ns,
            "chunk",
            fn2(|_, size: u64, xs: Vec<A>| -> Result<Vec<Vec<A>>, Error> {
                if size == 0 {
                    return Err(Error::Custom {
                        error: "Chunk size must be greater than zero".to_string(),
                        trace: Default::default(),
                    });
                }

                let chunk_size = size as usize;
                let mut chunks: Vec<Vec<A>> = Vec::new();
                let mut current: Vec<A> = Vec::with_capacity(chunk_size);

                for value in xs {
                    current.push(value);
                    if current.len() == chunk_size {
                        chunks.push(mem::take(&mut current));
                    }
                }

                if !current.is_empty() {
                    chunks.push(current);
                }

                Ok(chunks)
            }),
        );

        this.register(
            ns,
            "foldl",
            fn_async3(|ctx, f: Func<A, Func<B, A>>, base: A, xs: Vec<B>| {
                Box::pin(async move {
                    let mut res = base;
                    for x in xs {
                        let ares1 = apply(ctx, &f, &res, None).await?;
                        let ares2 = apply(ctx, &ares1, &x, None).await?;
                        res = A(ares2)
                    }
                    Ok(res)
                })
            }),
        );

        this.register(
            ns,
            "foldr",
            fn_async3(|ctx, f: Func<A, Func<B, B>>, base: B, xs: Vec<A>| {
                Box::pin(async move {
                    let mut res = base;
                    for x in xs.iter().rev() {
                        let ares1 = apply(ctx, &f, x, None).await?;
                        let ares2 = apply(ctx, &ares1, &res, None).await?;
                        res = B(ares2);
                    }
                    Ok(res)
                })
            }),
        );

        this.register(
            ns,
            "flatten",
            fn1(|_, nested: Vec<Vec<A>>| {
                let mut flattened: Vec<A> = Vec::new();

                for sublist in nested {
                    for item in sublist {
                        flattened.push(item);
                    }
                }

                Ok(flattened)
            }),
        );
        this.set_docs(
            ns,
            "flatten",
            "Lists",
            concat!(
                "Converts a list of lists into a list\n\n",
                "Example:\n\n",
                "```\n",
                "flatten [[1, 2, 3], [], [4, 5]]\n",
                "```\n\n",
                "Returns the value: `[1, 2, 3, 4, 5]`",
            ),
        );

        this.register(
            ns,
            "match_result",
            fn_async3(
                |ctx, r: Result<A, B>, f_ok: Func<A, C>, f_err: Func<B, C>| {
                    Box::pin(async move {
                        match r {
                            Ok(v) => Ok(C(apply(ctx, &f_ok, v, None).await?)),
                            Err(e) => Ok(C(apply(ctx, &f_err, e, None).await?)),
                        }
                    })
                },
            ),
        );

        this.register(
            ns,
            "match_option",
            fn_async3(
                |ctx, o: Option<A>, f_some: Func<A, B>, f_none: Func<(), B>| {
                    Box::pin(async move {
                        match o {
                            Some(v) => Ok(B(apply(ctx, &f_some, v, None).await?)),
                            None => {
                                let empty = Arc::new(Expr::Tuple(Span::default(), vec![]));
                                Ok(B(apply(ctx, &f_none, empty, None).await?))
                            }
                        }
                    })
                },
            ),
        );

        this.register(
            ns,
            ".",
            fn_async3(|ctx, f: Func<B, C>, g: Func<A, B>, x: A| {
                Box::pin(async move {
                    let x = apply(ctx, &g, &x, None).await?;
                    let x = apply(ctx, &f, &x, None).await?;
                    Ok(C(x))
                })
            }),
        );

        // Result
        this.register(ns, "Ok", fn1(|_, x: A| Ok(Ok::<A, B>(x))));
        this.register(ns, "Err", fn1(|_, x: B| Ok(Err::<A, B>(x))));
        this.register(ns, "is_ok", fn1(|_ctx, x: Result<A, E>| Ok(x.is_ok())));
        this.register(ns, "is_err", fn1(|_ctx, x: Result<A, E>| Ok(x.is_err())));
        this.register(
            ns,
            "map",
            fn_async2(|ctx, f: Func<A, B>, x: Result<A, E>| {
                Box::pin(async move {
                    match x {
                        Ok(x) => Ok(Ok(B(apply(ctx, &f, &x, None).await?))),
                        Err(e) => Ok(Err(e)),
                    }
                })
            }),
        );
        this.register(
            ns,
            "and_then",
            fn_async2(|ctx, f: Func<A, Result<B, E>>, x: Result<A, E>| {
                Box::pin(async move {
                    match x {
                        Ok(x) => Ok(Result::<B, E>::try_decode(
                            &apply(ctx, &f, &x, None).await?,
                        )?),
                        Err(e) => Ok(Err(e)),
                    }
                })
            }),
        );
        this.register(
            ns,
            "or_else",
            fn_async2(|ctx, f: Func<E, Result<A, F>>, x: Result<A, E>| {
                Box::pin(async move {
                    match x {
                        Ok(x) => Ok(Ok(x)),
                        Err(x) => Ok(Result::<A, F>::try_decode(
                            &apply(ctx, &f, &x, None).await?,
                        )?),
                    }
                })
            }),
        );
        this.register(
            ns,
            "unwrap_or_else",
            fn_async2(|ctx, f: Func<E, A>, x: Result<A, E>| {
                Box::pin(async move {
                    match x {
                        Ok(x) => Ok(x),
                        Err(x) => Ok(A(apply(ctx, &f, &x, None).await?)),
                    }
                })
            }),
        );
        this.register(
            ns,
            "unwrap",
            fn_async1(|_ctx, x: Result<A, E>| {
                Box::pin(async move {
                    match x {
                        Ok(x) => Ok(x),
                        Err(e) => Err(Error::Custom {
                            error: format!(
                                "unwrap called with Err: {}",
                                e.try_encode(Span::default())?
                            ),
                            trace: Default::default(),
                        }),
                    }
                })
            }),
        );

        // Option
        this.register(ns, "None", fn0(|_| Ok(None::<A>)));
        this.register(ns, "Some", fn1(|_, x: A| Ok(Some(x))));
        this.register(ns, "is_some", fn1(|_ctx, x: Option<A>| Ok(x.is_some())));
        this.register(ns, "is_none", fn1(|_ctx, x: Option<A>| Ok(x.is_none())));
        this.register(
            ns,
            "map",
            fn_async2(|ctx, f: Func<A, B>, x: Option<A>| {
                Box::pin(async move {
                    match x {
                        Some(x) => Ok(Some(B(apply(ctx, &f, &x, None).await?))),
                        None => Ok(None),
                    }
                })
            }),
        );
        this.register(
            ns,
            "and_then",
            fn_async2(|ctx, f: Func<A, Option<B>>, x: Option<A>| {
                Box::pin(async move {
                    match x {
                        Some(x) => Ok(Option::<B>::try_decode(&apply(ctx, &f, &x, None).await?)?),
                        None => Ok(None),
                    }
                })
            }),
        );
        this.register(
            ns,
            "or_else",
            fn_async2(|ctx, f: Func<(), Option<A>>, x: Option<A>| {
                Box::pin(async move {
                    match x {
                        Some(x) => Ok(Some(x)),
                        None => {
                            let x = Arc::new(Expr::Tuple(Span::default(), vec![]));
                            let res = apply(ctx, &f, &x, None).await?;
                            Ok(Option::<A>::try_decode(&res)?)
                        }
                    }
                })
            }),
        );
        this.register(
            ns,
            "unwrap_or_else",
            fn_async2(|ctx, f: Func<(), A>, x: Option<A>| {
                Box::pin(async move {
                    match x {
                        Some(x) => Ok(x),
                        None => {
                            let x = Arc::new(Expr::Tuple(Span::default(), vec![]));
                            let res = apply(ctx, &f, &x, None).await?;
                            Ok(A(res))
                        }
                    }
                })
            }),
        );
        this.register(
            ns,
            "unwrap",
            fn_async1(|_ctx, x: Option<A>| {
                Box::pin(async move {
                    match x {
                        Some(x) => Ok(x),
                        None => Err(Error::Custom {
                            error: "unwrap called with None".to_string(),
                            trace: Default::default(),
                        }),
                    }
                })
            }),
        );

        // Uuid
        this.register(ns, "string", fn1(|_, x: Uuid| Ok(format!("{}", x))));
        this.register(ns, "random_uuid", fn0(|_| Ok(Uuid::new_v4())));
        this.register(
            ns,
            "uuid",
            fn1(|_, x: String| -> Result<Uuid, Error> {
                Uuid::from_str(&x).map_err(|_| Error::Custom {
                    error: format!("Invalid UUID {:?}", x),
                    trace: Default::default(),
                })
            }),
        );

        // DateTime
        this.register(
            ns,
            "string",
            fn1(|_, x: DateTime<Utc>| Ok(format!("{}", x))),
        );
        this.register(ns, "now", fn0(|_| Ok(Utc::now())));

        this.register(
            ns,
            "list_range",
            fn3(
                |_, start: u64, end: u64, step: Option<u64>| -> Result<Vec<u64>, Error> {
                    let step = step.unwrap_or(1);
                    if step == 0 {
                        return Err(Error::Custom {
                            error: "Step cannot be zero".to_string(),
                            trace: Default::default(),
                        });
                    }

                    Ok((start..end).step_by(step as usize).collect())
                },
            ),
        );

        this.register(
            ns,
            "list_min",
            fn1(|_, values: Vec<i64>| -> Result<i64, Error> {
                if values.is_empty() {
                    return Err("List is empty".into());
                }

                let mut res = values[0];
                for v in &values[1..] {
                    if *v < res {
                        res = *v;
                    }
                }
                Ok(res)
            }),
        );

        this.register(
            ns,
            "list_min",
            fn1(|_, values: Vec<u64>| -> Result<u64, Error> {
                if values.is_empty() {
                    return Err("List is empty".into());
                }

                let mut res = values[0];
                for v in &values[1..] {
                    if *v < res {
                        res = *v;
                    }
                }
                Ok(res)
            }),
        );

        this.register(
            ns,
            "list_min",
            fn1(|_, values: Vec<f64>| -> Result<f64, Error> {
                if values.is_empty() {
                    return Err("List is empty".into());
                }

                let mut res = values[0];
                for v in &values[1..] {
                    if *v < res {
                        res = *v;
                    }
                }
                Ok(res)
            }),
        );
        this.set_docs(
            ns,
            "list_min",
            "Lists",
            concat!(
                "Returns the minimum value in a list. ",
                "If the list is empty, an error will be generated\n\n",
                "Example:\n\n",
                "```\n",
                "list_max [5, 2, 8, 3]\n",
                "```\n\n",
                "Returns the value: `2`"
            ),
        );

        this.register(
            ns,
            "list_max",
            fn1(|_, values: Vec<i64>| -> Result<i64, Error> {
                if values.is_empty() {
                    return Err("List is empty".into());
                }

                let mut res = values[0];
                for v in &values[1..] {
                    if *v > res {
                        res = *v;
                    }
                }
                Ok(res)
            }),
        );

        this.register(
            ns,
            "list_max",
            fn1(|_, values: Vec<u64>| -> Result<u64, Error> {
                if values.is_empty() {
                    return Err("List is empty".into());
                }

                let mut res = values[0];
                for v in &values[1..] {
                    if *v > res {
                        res = *v;
                    }
                }
                Ok(res)
            }),
        );

        this.register(
            ns,
            "list_max",
            fn1(|_, values: Vec<f64>| -> Result<f64, Error> {
                if values.is_empty() {
                    return Err("List is empty".into());
                }

                let mut res = values[0];
                for v in &values[1..] {
                    if *v > res {
                        res = *v;
                    }
                }
                Ok(res)
            }),
        );
        this.set_docs(
            ns,
            "list_max",
            "Lists",
            concat!(
                "Returns the maximum value in a list. ",
                "If the list is empty, an error will be generated\n\n",
                "Example:\n\n",
                "```\n",
                "list_max [5, 2, 8, 3]\n",
                "```\n\n",
                "Returns the value: `8`"
            ),
        );

        this.register(
            ns,
            "list_sum",
            fn1(|_, values: Vec<i64>| -> Result<i64, Error> {
                let mut sum = 0;
                for v in values {
                    sum += v;
                }
                Ok(sum)
            }),
        );

        this.register(
            ns,
            "list_sum",
            fn1(|_, values: Vec<u64>| -> Result<u64, Error> {
                let mut sum = 0;
                for v in values {
                    sum += v;
                }
                Ok(sum)
            }),
        );

        this.register(
            ns,
            "list_sum",
            fn1(|_, values: Vec<f64>| -> Result<f64, Error> {
                let mut sum = 0.0;
                for v in values {
                    sum += v;
                }
                Ok(sum)
            }),
        );

        this.register(
            ns,
            "list_avg",
            fn1(|_, values: Vec<i64>| -> Result<f64, Error> {
                let count = values.len() as f64;
                let mut sum = 0.0;
                for v in values {
                    sum += v as f64;
                }
                Ok(sum / count)
            }),
        );

        this.register(
            ns,
            "list_avg",
            fn1(|_, values: Vec<u64>| -> Result<f64, Error> {
                let count = values.len() as f64;
                let mut sum = 0.0;
                for v in values {
                    sum += v as f64;
                }
                Ok(sum / count)
            }),
        );

        this.register(
            ns,
            "list_avg",
            fn1(|_, values: Vec<f64>| -> Result<f64, Error> {
                let count = values.len() as f64;
                let mut sum = 0.0;
                for v in values {
                    sum += v;
                }
                Ok(sum / count)
            }),
        );

        this.register(
            ns,
            "list_max_by",
            fn_async2(|ctx, f: Func<A, i64>, xs: Vec<A>| {
                Box::pin(async move {
                    if xs.is_empty() {
                        return Err("List is empty".into());
                    }

                    let mut max_elem = xs[0].clone();
                    let mut max_key = i64::try_decode(&apply(ctx, &f, &max_elem, None).await?)?;

                    for x in &xs[1..] {
                        let key = i64::try_decode(&apply(ctx, &f, x, None).await?)?;
                        if key > max_key {
                            max_key = key;
                            max_elem = x.clone();
                        }
                    }
                    Ok(max_elem)
                })
            }),
        );

        this.register(
            ns,
            "list_max_by",
            fn_async2(|ctx, f: Func<A, u64>, xs: Vec<A>| {
                Box::pin(async move {
                    if xs.is_empty() {
                        return Err("List is empty".into());
                    }

                    let mut max_elem = xs[0].clone();
                    let mut max_key = u64::try_decode(&apply(ctx, &f, &max_elem, None).await?)?;

                    for x in &xs[1..] {
                        let key = u64::try_decode(&apply(ctx, &f, x, None).await?)?;
                        if key > max_key {
                            max_key = key;
                            max_elem = x.clone();
                        }
                    }
                    Ok(max_elem)
                })
            }),
        );

        this.register(
            ns,
            "list_max_by",
            fn_async2(|ctx, f: Func<A, f64>, xs: Vec<A>| {
                Box::pin(async move {
                    if xs.is_empty() {
                        return Err("List is empty".into());
                    }

                    let mut max_elem = xs[0].clone();
                    let mut max_key = f64::try_decode(&apply(ctx, &f, &max_elem, None).await?)?;

                    for x in &xs[1..] {
                        let key = f64::try_decode(&apply(ctx, &f, x, None).await?)?;
                        if key > max_key {
                            max_key = key;
                            max_elem = x.clone();
                        }
                    }
                    Ok(max_elem)
                })
            }),
        );

        Ok(this)
    }

    pub fn register_elem_functions(&mut self, namespace: &Namespace) {
        // Note: Each of these has a unique name, because we don't currently support overloading
        // of functions containing generic types.
        for tuple_len in 1..=MAX_TUPLE_LEN {
            let mut element_types: Vec<Arc<Type>> = Vec::new();
            for i in 0..tuple_len {
                element_types.push(Arc::new(Type::UnresolvedVar(format!("T{}", i))));
            }
            let tuple_type = Arc::new(Type::Tuple(element_types.clone()));

            for tuple_index in 0..tuple_len {
                let fun_name = format!("elem{}", tuple_index);
                let fun_type = Type::arrow(tuple_type.clone(), element_types[tuple_index].clone());
                let tuple_type = tuple_type.clone();
                self.register_fn_core_with_name(
                    namespace,
                    &fun_name,
                    fun_type,
                    Box::new(move |_, args| {
                        let tuple_type = tuple_type.clone();
                        Box::pin(async move {
                            if let Some(arg) = args.first() {
                                match &**arg {
                                    Expr::Tuple(_, elems) if elems.len() == tuple_len => {
                                        Ok(elems[tuple_index].clone())
                                    }
                                    _ => Err(Error::ExpectedTypeGotValue {
                                        expected: tuple_type.clone(),
                                        got: arg.clone(),
                                        trace: Default::default(),
                                    }),
                                }
                            } else {
                                Err(Error::MissingArgument {
                                    argument: 0,
                                    trace: Default::default(),
                                })
                            }
                        })
                    }),
                )
                .unwrap();
            }
        }
    }

    pub fn register(&mut self, ns: &Namespace, n: impl ToString, tfun: TypedFunction<State>) {
        let n = n.to_string();
        register_fn_core(self, ns, &n, tfun.t.clone()).unwrap();
        self.ftable.add_fn(ns, n, Box::new(tfun.t), tfun.f).unwrap();
    }

    pub fn register_fn_core_with_name(
        &mut self,
        ns: &Namespace,
        name: &str,
        t: Arc<Type>,
        f: FtableFn<State>,
    ) -> Result<(), Error> {
        register_fn_core(self, ns, name, t.clone())?;
        self.ftable.add_fn(ns, name, Box::new(t), f)?;
        Ok(())
    }

    pub fn register_adt(
        &mut self,
        ns: &Namespace,
        adt_type: &Arc<Type>,
        prefix: Option<&str>,
        defaults: Option<&BTreeMap<String, FtableFn<State>>>,
    ) -> Result<(), Error> {
        let Type::ADT(adt) = &**adt_type else {
            panic!("register_adt) called with non-ADT type: {}", adt_type);
        };

        let full_adt_name = make_full_name(prefix, &adt.name);
        if let Some(existing) = self.adts.get(&full_adt_name) {
            if existing == adt {
                // An identical ADT already exists; allow this
                return Ok(());
            } else {
                // A different ADT with the same name is already registered
                return Err(Error::ADTNameConflict {
                    name: full_adt_name.clone(),
                    new: Arc::new(adt.clone()),
                    existing: Arc::new(existing.clone()),
                    trace: Default::default(),
                });
            }
        }
        self.adts.insert(full_adt_name.clone(), adt.clone());

        if adt.variants.len() != 1 && defaults.is_some() {
            panic!(
                "register_adt) called with defaults and multiple variants {}",
                adt_type
            );
        }

        if adt.variants.is_empty() {
            self.register_adt_variant(
                ns,
                adt_type,
                &adt.name,
                &None,
                &full_adt_name,
                defaults,
                false,
            )?;
        } else if adt.variants.len() == 1 && adt.variants[0].name == adt.name {
            // Only register accessors if there is exactly one variant
            self.register_adt_variant(
                ns,
                adt_type,
                &adt.name,
                &adt.variants[0].t,
                &full_adt_name,
                defaults,
                true,
            )?;
        } else {
            for variant in &adt.variants {
                let constructor_name = make_full_name(Some(&full_adt_name), &variant.name);
                self.register_adt_variant(
                    ns,
                    adt_type,
                    &variant.name,
                    &variant.t,
                    &constructor_name,
                    defaults,
                    false,
                )?;
            }
        }

        // Register ADT-specific equality and inequality overloads
        // This enables type-safe equality comparisons for enum variants and structs,
        // e.g., `(kind i) == BindingSiteInteractionKind::HydrogenBond`
        // Note: For ADTs with float fields, this uses exact bitwise comparison (not approximate)
        //
        // Only register == and != once per ADT base name (without prefix) to avoid conflicts
        // when multiple modules expose ADTs with the same name.
        if !self.equality_registered.contains(&adt.name) {
            use rex_type_system::types::TypeCon;

            let bool_t = Arc::new(Type::Con(TypeCon::Bool));
            let eq_t = Type::arrow(
                adt_type.clone(),
                Type::arrow(adt_type.clone(), bool_t.clone()),
            );
            let ne_t = Type::arrow(
                adt_type.clone(),
                Type::arrow(adt_type.clone(), bool_t.clone()),
            );

            // == overload (structural equality, ignoring spans)
            // Use the same approach as register_accessors: register the type first,
            // and ignore if an identical overload already exists (happens when multiple
            // modules expose ADTs with the same name)
            match register_fn_core(self, ns, "==", eq_t.clone()) {
                Ok(()) => {
                    // Only add the implementation if type registration succeeded
                    self.ftable.add_fn(
                        ns,
                        "==",
                        Box::new(eq_t),
                        Box::new(|_, args: &Vec<Arc<Expr>>| {
                            Box::pin(async move {
                                // Compare values ignoring source location spans
                                // This performs deep structural comparison: variant name + all fields
                                let lhs = args[0].reset_spans();
                                let rhs = args[1].reset_spans();
                                Ok(Arc::new(Expr::Bool(Span::default(), lhs == rhs)))
                            })
                        }),
                    )?;
                }
                Err(Error::OverlappingFunctions { overlap, .. }) if overlap.t1 == overlap.t2 => {
                    // Ignore this case; it can happen if there are multiple ADTs imported
                    // from different modules that have the same name but different prefixes
                }
                Err(e) => return Err(e),
            }

            // != overload (structural inequality, ignoring spans)
            match register_fn_core(self, ns, "!=", ne_t.clone()) {
                Ok(()) => {
                    // Only add the implementation if type registration succeeded
                    self.ftable.add_fn(
                        ns,
                        "!=",
                        Box::new(ne_t),
                        Box::new(|_, args: &Vec<Arc<Expr>>| {
                            Box::pin(async move {
                                // Compare values ignoring source location spans
                                let lhs = args[0].reset_spans();
                                let rhs = args[1].reset_spans();
                                Ok(Arc::new(Expr::Bool(Span::default(), lhs != rhs)))
                            })
                        }),
                    )?;
                }
                Err(Error::OverlappingFunctions { overlap, .. }) if overlap.t1 == overlap.t2 => {
                    // Ignore this case; it can happen if there are multiple ADTs imported
                    // from different modules that have the same name but different prefixes
                }
                Err(e) => return Err(e),
            }

            // Mark this ADT name as having equality registered
            self.equality_registered.insert(adt.name.clone());
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn register_adt_variant(
        &mut self,
        ns: &Namespace,
        adt_type: &Arc<Type>,
        variant_name: &str,
        variant_type: &Option<Arc<Type>>,
        constructor_name: &str,
        defaults: Option<&BTreeMap<String, FtableFn<State>>>,
        accessors: bool,
    ) -> Result<(), Error> {
        let base_name = variant_name.to_string();

        // TODO: Avoid cloning args in the functions. This applies more generally across
        // the evaluation code. We should either pass owned Expr values to the functions
        // or use Rc<Expr> to make cloning cheap.

        // We do not support multiple ADTs with overlapping constructor names. The reason
        // is because in the general case, each constructor may have a different number of
        // arguments, so we cannot consider them to be overloaded functions. Haskell has the
        // same restriction.
        if self.ftable.contains(constructor_name) {
            panic!("Duplicate constructor name: {}", constructor_name);
        }

        // The functions we register here work directly with Expr values; there is no need
        // to convert to and from the corresponding native Rust type.
        match (variant_type.as_ref().map(|t| &**t), defaults) {
            (Some(Type::Tuple(fields)), _) => {
                let mut fun_type = adt_type.clone();
                for field in fields.iter().rev() {
                    fun_type = Type::arrow(field.clone(), fun_type);
                }
                self.register_fn_core_with_name(
                    ns,
                    constructor_name,
                    fun_type,
                    Box::new(move |_, args| {
                        let base_name = base_name.clone();
                        Box::pin(async move {
                            Ok(Arc::new(Expr::Named(
                                Span::default(),
                                base_name,
                                Some(Arc::new(Expr::Tuple(Span::default(), args.clone()))),
                            )))
                        })
                    }),
                )
            }
            (Some(Type::Dict(entries)), Some(defaults)) => {
                let mut entries_without_defaults: BTreeMap<String, Arc<Type>> = BTreeMap::new();
                for (k, v) in entries.iter() {
                    if !defaults.contains_key(k) {
                        entries_without_defaults.insert(k.clone(), v.clone());
                    }
                }
                let t = Arc::new(Type::Dict(entries_without_defaults));

                let fun_type = Type::arrow(t.clone(), adt_type.clone());
                let defaults: BTreeMap<String, FtableFn<State>> = (*defaults).clone();
                let base_name1 = base_name.clone();
                self.register_fn_core_with_name(
                    ns,
                    constructor_name,
                    fun_type,
                    Box::new(move |ctx, args| {
                        let base_name = base_name1.clone();
                        let defaults = defaults.clone();
                        Box::pin(async move {
                            let mut val = args[0].clone();

                            if let Expr::Dict(span, entries) = &*val {
                                let mut new_entries: BTreeMap<String, Arc<Expr>> = entries.clone();
                                for (k, vf) in defaults.iter() {
                                    let v = vf(ctx, &vec![]).await?;
                                    new_entries.insert(k.clone(), v);
                                }
                                val = Arc::new(Expr::Dict(*span, new_entries));
                            }

                            Ok(Arc::new(Expr::Named(Span::default(), base_name, Some(val))))
                        })
                    }),
                )?;

                if accessors {
                    self.register_accessors(ns, adt_type, entries)?;
                }
                Ok(())
            }
            _ => {
                if let Some(t) = variant_type {
                    let fun_type = Type::arrow(t.clone(), adt_type.clone());
                    self.register_fn_core_with_name(
                        ns,
                        constructor_name,
                        fun_type,
                        Box::new(move |_, args| {
                            let base_name = base_name.clone();
                            Box::pin(async move {
                                let val = args[0].clone();
                                Ok(Arc::new(Expr::Named(Span::default(), base_name, Some(val))))
                            })
                        }),
                    )?;

                    if accessors {
                        if let Type::Dict(entries) = &**t {
                            self.register_accessors(ns, adt_type, entries)?;
                        }
                    }
                    Ok(())
                } else {
                    self.register_fn_core_with_name(
                        ns,
                        constructor_name,
                        adt_type.clone(),
                        Box::new(move |_, _| {
                            let base_name = base_name.clone();
                            Box::pin(async move {
                                Ok(Arc::new(Expr::Named(Span::default(), base_name, None)))
                            })
                        }),
                    )
                }
            }
        }
    }

    fn register_accessors(
        &mut self,
        ns: &Namespace,
        adt_type: &Arc<Type>,
        entries: &BTreeMap<String, Arc<Type>>,
    ) -> Result<(), Error> {
        for (entry_key, entry_type) in entries.iter() {
            let this_accessor_fun_type = Type::arrow(adt_type.clone(), entry_type.clone());

            // Register the type
            match register_fn_core(self, ns, entry_key, this_accessor_fun_type.clone()) {
                Ok(()) => {}
                Err(Error::OverlappingFunctions { .. }) => {
                    // Ignore all accessor overlaps; they can happen when:
                    // 1. Multiple ADTs have the same field name (e.g., "id", "name")
                    // 2. Multiple modules expose ADTs with the same name but different definitions
                    // The accessor implementation is generic and works for all cases.
                }
                Err(e) => return Err(e),
            }

            // Register the implementation, if one does not already exist
            if !self.accessors.contains(entry_key) {
                self.accessors.insert(entry_key.to_string());
                self.ftable.add_fn(
                    ns,
                    entry_key,
                    Box::new(Accessor),
                    make_accessor_fn(entry_key),
                )?;
            }
        }
        Ok(())
    }

    pub fn build(self) -> (Ftable<State>, TypeEnv) {
        let mut ftenv = HashMap::new();
        for (name, entry) in self.builtins.into_iter() {
            ftenv.insert(name.clone(), entry.into_type_schemes());
        }
        (self.ftable, ftenv)
    }
}

struct Accessor;

impl fmt::Display for Accessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        "Accessor".fmt(f)
    }
}

impl Dispatch for Accessor {
    fn num_params(&self) -> usize {
        1
    }

    fn maybe_accepts_args(&self, args: &[Arc<Expr>]) -> bool {
        args.len() == 1 && matches!(&*args[0], Expr::Named(..))
    }
}

fn make_full_name(prefix: Option<&str>, name: &str) -> String {
    match prefix {
        Some(prefix) => format!("{}::{}", prefix, name),
        None => name.to_string(),
    }
}

fn make_accessor_fn<State>(entry_key: &str) -> FtableFn<State>
where
    State: Clone + Sync + 'static,
{
    let entry_key = entry_key.to_string();
    Box::new(move |_, args: &Vec<Arc<Expr>>| {
        let entry_key = entry_key.clone();
        Box::pin(async move {
            match &*args[0] {
                Expr::Named(_, _, Some(inner)) => match &**inner {
                    Expr::Dict(_, entries) => match entries.get(&entry_key) {
                        Some(v) => Ok(v.clone()),
                        None => Err(Error::Custom {
                            error: format!("Missing entry {:?}", entry_key),
                            trace: Default::default(),
                        }),
                    },
                    _ => Err(Error::Custom {
                        error: "Expected a dict".to_string(),
                        trace: Default::default(),
                    }),
                },
                _ => Err(Error::Custom {
                    error: "Expected a Named".to_string(),
                    trace: Default::default(),
                }),
            }
        })
    })
}
