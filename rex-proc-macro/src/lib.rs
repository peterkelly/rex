#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use proc_macro::TokenStream;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{ToTokens, format_ident, quote};
use std::collections::BTreeMap;
use syn::{
    Attribute, Data, DeriveInput, Error, Expr, Fields, FnArg, GenericArgument, Generics, Ident,
    Item, ItemFn, ItemMod, Lit, LitStr, Meta, Pat, PathArguments, Token, Type, parse::Parse,
    parse::ParseStream, parse_quote, punctuated::Punctuated, spanned::Spanned,
};

#[proc_macro_attribute]
pub fn export(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match syn::parse(args) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let function: ItemFn = match syn::parse(input) {
        Ok(function) => function,
        Err(error) => return error.to_compile_error().into(),
    };
    match expand_export(args, function) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match syn::parse(args) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let module: ItemMod = match syn::parse(input) {
        Ok(module) => module,
        Err(error) => return error.to_compile_error().into(),
    };
    match expand_module(args, module) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(Rex, attributes(rex, serde))]
pub fn derive_rex(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = match syn::parse(input) {
        Ok(ast) => ast,
        Err(e) => return e.to_compile_error().into(),
    };
    match expand(&ast) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[derive(Default)]
struct RegistrationArgs {
    name: Option<LitStr>,
}

impl Parse for RegistrationArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let metas = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
        let mut args = Self::default();
        for meta in metas {
            match meta {
                Meta::NameValue(meta) if meta.path.is_ident("name") => {
                    if args.name.is_some() {
                        return Err(Error::new(meta.span(), "duplicate `name` argument"));
                    }
                    let Expr::Lit(expr) = meta.value else {
                        return Err(Error::new(meta.span(), "`name` must be a string literal"));
                    };
                    let Lit::Str(name) = expr.lit else {
                        return Err(Error::new(
                            expr.lit.span(),
                            "`name` must be a string literal",
                        ));
                    };
                    args.name = Some(name);
                }
                other => {
                    return Err(Error::new(other.span(), "expected `name = \"...\"`"));
                }
            }
        }
        Ok(args)
    }
}

fn exported_state_type(function: &ItemFn) -> Result<Type, Error> {
    let first = function.sig.inputs.first().ok_or_else(|| {
        Error::new(
            function.sig.span(),
            "a Rex export must take an owned `State` as its first parameter",
        )
    })?;
    let FnArg::Typed(first) = first else {
        return Err(Error::new(
            first.span(),
            "methods cannot be Rex exports; use a free function whose first parameter is an owned `State`",
        ));
    };
    if let Type::Reference(reference) = first.ty.as_ref() {
        return Err(Error::new(
            reference.span(),
            "a Rex export's first parameter must be an owned `State`, not a reference",
        ));
    }
    Ok(first.ty.as_ref().clone())
}

fn exported_param_names(function: &ItemFn) -> Result<Vec<String>, Error> {
    let mut names = Vec::new();
    for argument in function.sig.inputs.iter().skip(1) {
        let FnArg::Typed(argument) = argument else {
            return Err(Error::new(argument.span(), "methods cannot be Rex exports"));
        };
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(Error::new(
                argument.pat.span(),
                "Rex export parameters must use simple identifier patterns",
            ));
        };
        if pattern.subpat.is_some() {
            return Err(Error::new(
                pattern.span(),
                "Rex export parameters cannot use subpatterns",
            ));
        }
        let name = pattern.ident.to_string();
        names.push(name.strip_prefix("r#").unwrap_or(&name).to_string());
    }
    Ok(names)
}

fn expand_export(args: RegistrationArgs, function: ItemFn) -> Result<TokenStream2, Error> {
    if !function.sig.generics.params.is_empty() || function.sig.generics.where_clause.is_some() {
        return Err(Error::new(
            function.sig.generics.span(),
            "generic functions cannot be registered as Rex exports",
        ));
    }
    if function.sig.constness.is_some() || function.sig.unsafety.is_some() {
        return Err(Error::new(
            function.sig.span(),
            "const or unsafe functions cannot be registered as Rex exports",
        ));
    }

    let state = exported_state_type(&function)?;
    let param_names = exported_param_names(&function)?;
    let docs = docs_from_attrs(&function.attrs)?;
    let function_name = &function.sig.ident;
    let helper_name = format_ident!("{}_rex_export", function_name);
    let export_name = args
        .name
        .map(|name| name.value())
        .unwrap_or_else(|| function_name.to_string());
    let constructor = if function.sig.asyncness.is_some() {
        quote!(::rex::engine::Export::<#state>::from_async_handler)
    } else {
        quote!(::rex::engine::Export::<#state>::from_handler)
    };
    let attach_docs = docs.map(|docs| {
        quote! {
            __rex_export = __rex_export.with_docs(#docs);
        }
    });

    Ok(quote! {
        #function

        #[doc(hidden)]
        pub fn #helper_name() -> Result<
            ::rex::engine::Export<#state>,
            ::rex::engine::EngineError,
        > {
            let __rex_param_names: ::std::vec::Vec<&str> =
                ::std::vec![#(#param_names),*];
            let mut __rex_export = #constructor(#export_name, #function_name)?
                .with_param_names(__rex_param_names)?;
            #attach_docs
            Ok(__rex_export)
        }
    })
}

fn is_rex_export_attr(attr: &Attribute) -> bool {
    let segments = attr.path().segments.iter().collect::<Vec<_>>();
    segments.len() == 2 && segments[0].ident == "rex" && segments[1].ident == "export"
}

fn has_rex_export_marker(attrs: &[Attribute]) -> Result<bool, Error> {
    for attr in attrs {
        if !attr.path().is_ident("rex") {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        let metas = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        if metas
            .iter()
            .any(|meta| matches!(meta, Meta::Path(path) if path.is_ident("export")))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn expand_module(args: RegistrationArgs, mut module: ItemMod) -> Result<TokenStream2, Error> {
    let module_name = args.name.ok_or_else(|| {
        Error::new(
            module.ident.span(),
            "a Rex module requires `name = \"...\"`",
        )
    })?;
    let module_docs = docs_from_attrs(&module.attrs)?;
    let module_span = module.span();
    let (_, items) = module
        .content
        .as_mut()
        .ok_or_else(|| Error::new(module_span, "`#[rex::module]` requires an inline module"))?;
    if items
        .iter()
        .any(|item| matches!(item, Item::Fn(function) if function.sig.ident == "rex_module"))
    {
        return Err(Error::new(
            module.ident.span(),
            "a registered Rex module cannot define its own `rex_module` function",
        ));
    }

    let mut state: Option<(String, Type)> = None;
    let mut export_helpers = Vec::new();
    let mut exported_types = Vec::new();
    for item in items.iter() {
        match item {
            Item::Fn(function) if function.attrs.iter().any(is_rex_export_attr) => {
                let function_state = exported_state_type(function)?;
                let state_tokens = function_state.to_token_stream().to_string();
                if let Some((expected, _)) = &state {
                    if expected != &state_tokens {
                        return Err(Error::new(
                            function_state.span(),
                            "all functions in a registered Rex module must use the same state type",
                        ));
                    }
                } else {
                    state = Some((state_tokens, function_state));
                }
                export_helpers.push(format_ident!("{}_rex_export", function.sig.ident));
            }
            Item::Struct(item) if has_rex_export_marker(&item.attrs)? => {
                if !item.generics.params.is_empty() {
                    return Err(Error::new(
                        item.generics.span(),
                        "a generic ADT exported by a module needs concrete type arguments",
                    ));
                }
                exported_types.push(item.ident.clone());
            }
            Item::Enum(item) if has_rex_export_marker(&item.attrs)? => {
                if !item.generics.params.is_empty() {
                    return Err(Error::new(
                        item.generics.span(),
                        "a generic ADT exported by a module needs concrete type arguments",
                    ));
                }
                exported_types.push(item.ident.clone());
            }
            _ => {}
        }
    }
    let state = state
        .map(|(_, state)| state)
        .unwrap_or_else(|| parse_quote!(()));
    let module_docs = docs_expr(module_docs);
    let factory: Item = syn::parse2(quote! {
        /// Build this Rust module's documented Rex registration.
        pub fn rex_module() -> Result<
            ::rex::engine::Module<#state>,
            ::rex::engine::EngineError,
        > {
            let mut __rex_module = ::rex::engine::Module::<#state>::new(
                #module_name,
                #module_docs,
            );
            #(__rex_module.add_rex_adt::<#exported_types>()?;)*
            #(__rex_module.add_export(#export_helpers()?)?;)*
            Ok(__rex_module)
        }
    })?;
    items.push(factory);
    Ok(module.into_token_stream())
}

struct DeriveOptions {
    name: String,
}

fn expand(ast: &DeriveInput) -> Result<TokenStream2, Error> {
    if ast.generics.lifetimes().next().is_some() || ast.generics.const_params().next().is_some() {
        return Err(Error::new(
            ast.generics.span(),
            "`#[derive(Rex)]` only supports type parameters (no lifetimes or const generics)",
        ));
    }

    let opts = DeriveOptions {
        name: rex_name_from_attrs(&ast.attrs)?.unwrap_or_else(|| ast.ident.to_string()),
    };

    let rust_ident = &ast.ident;
    let type_name = opts.name;
    let type_param_idents: Vec<Ident> = ast
        .generics
        .type_params()
        .map(|p| p.ident.clone())
        .collect();
    let type_param_count = type_param_idents.len();

    let mut rex_type_generics = ast.generics.clone();
    add_bound_to_type_params(
        &mut rex_type_generics,
        parse_quote!(::rex::typesystem::RexType),
    );
    let (rex_type_impl_generics, rex_type_ty_generics, rex_type_where_clause) =
        rex_type_generics.split_for_impl();
    let rex_type_params = type_param_idents.iter().map(|ident| {
        quote! { <#ident as ::rex::typesystem::RexType>::rex_type() }
    });
    let rex_type_collect_family = adt_family_fn(ast, &type_name, &type_param_idents)?;
    let rex_type_impl = quote! {
        impl #rex_type_impl_generics ::rex::typesystem::RexType for #rust_ident #rex_type_ty_generics #rex_type_where_clause {
            fn rex_type() -> ::rex::typesystem::Type {
                let mut ty = ::rex::typesystem::Type::con(#type_name, #type_param_count);
                #( ty = ::rex::typesystem::Type::app(ty, #rex_type_params); )*
                ty
            }

            fn collect_rex_family(
                out: &mut ::std::vec::Vec<::rex::typesystem::AdtDecl>,
            ) -> Result<(), ::rex::typesystem::TypeError> {
                #rex_type_collect_family
            }
        }
    };
    let adt_decl_fn = adt_decl_fn(ast, &type_name, &type_param_idents)?;
    let mut rex_adt_generics = ast.generics.clone();
    add_bound_to_type_params(
        &mut rex_adt_generics,
        parse_quote!(::rex::typesystem::RexType),
    );
    let (rex_adt_impl_generics, rex_adt_ty_generics, rex_adt_where_clause) =
        rex_adt_generics.split_for_impl();
    let rex_adt_impl = quote! {
        impl #rex_adt_impl_generics ::rex::typesystem::RexAdt for #rust_ident #rex_adt_ty_generics #rex_adt_where_clause {
            fn rex_adt_decl() -> Result<::rex::typesystem::AdtDecl, ::rex::typesystem::TypeError> {
                #adt_decl_fn
            }
        }
    };
    let inject_fn = quote! {
        impl #rex_adt_impl_generics #rust_ident #rex_adt_ty_generics #rex_adt_where_clause {
            pub fn inject_rex<State: Clone + Send + Sync + 'static>(
                builder: &mut ::rex::engine::Builder<State>,
            ) -> Result<(), ::rex::engine::EngineError> {
                builder.inject_rex_adt::<Self>()
            }

            pub fn rex_adt_decl() -> Result<::rex::typesystem::AdtDecl, ::rex::engine::EngineError> {
                Ok(<Self as ::rex::typesystem::RexAdt>::rex_adt_decl()?)
            }

            pub fn rex_adt_family() -> Result<::std::vec::Vec<::rex::typesystem::AdtDecl>, ::rex::engine::EngineError> {
                Ok(<Self as ::rex::typesystem::RexAdt>::rex_adt_family()?)
            }

            pub fn inject_rex_with_default<State: Clone + Send + Sync + 'static>(
                builder: &mut ::rex::engine::Builder<State>,
            ) -> Result<(), ::rex::engine::EngineError>
            where
                Self: ::rex::engine::RexDefault<State>,
            {
                builder.inject_rex_adt::<Self>()?;
                builder.inject_rex_default_instance::<Self>()
            }

            pub fn inject_rex_with_constructor<State, Sig, H>(
                builder: &mut ::rex::engine::Builder<State>,
                constructor: H,
            ) -> Result<(), ::rex::engine::EngineError>
            where
                State: Clone + Send + Sync + 'static,
                H: ::rex::engine::HostFnSync<State, Sig>,
            {
                builder.inject_rex_adt::<Self>()?;
                let mut module = ::rex::engine::Module::global();
                module.export(#type_name, constructor)?;
                builder.inject_module(module)
            }
        }
    };

    let into_value_impl = into_value_impl(ast, &type_name)?;
    let from_value_impl = from_value_impl(ast, &type_name)?;

    Ok(quote! {
        #rex_type_impl
        #rex_adt_impl
        #inject_fn
        #into_value_impl
        #from_value_impl
    })
}

fn rex_name_from_attrs(attrs: &[Attribute]) -> Result<Option<String>, Error> {
    let mut name: Option<String> = None;
    for attr in attrs {
        if !attr.path().is_ident("rex") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                if name.is_some() {
                    return Err(meta.error("duplicate `name` argument"));
                }
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                name = Some(lit.value());
            }
            Ok(())
        })?;
    }
    Ok(name)
}

fn serde_rename_from_attrs(attrs: &[Attribute]) -> Result<Option<String>, Error> {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let mut rename: Option<String> = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                rename = Some(lit.value());
            } else if meta.path.is_ident("alias") {
                // Consume and ignore aliases so serde meta parsing doesn't fail.
                let value = meta.value()?;
                let _lit: LitStr = value.parse()?;
            } else if meta.path.is_ident("default") {
                // Consume and ignore defaults (function path as string literal).
                let value = meta.value()?;
                let _lit: LitStr = value.parse()?;
            }
            Ok(())
        })?;
        if rename.is_some() {
            return Ok(rename);
        }
    }
    Ok(None)
}

fn docs_from_attrs(attrs: &[Attribute]) -> Result<Option<String>, Error> {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        let Meta::NameValue(meta) = &attr.meta else {
            continue;
        };
        let Expr::Lit(expr) = &meta.value else {
            return Err(Error::new(
                meta.value.span(),
                "Rex documentation requires literal Rust doc comments",
            ));
        };
        let Lit::Str(value) = &expr.lit else {
            return Err(Error::new(
                expr.lit.span(),
                "Rex documentation requires string-valued Rust doc comments",
            ));
        };
        let line = value.value();
        lines.push(line.strip_prefix(' ').unwrap_or(&line).to_string());
    }
    if lines.is_empty() {
        Ok(None)
    } else {
        let docs = lines.join("\n").trim_matches('\n').to_string();
        Ok((!docs.is_empty()).then_some(docs))
    }
}

fn docs_expr(docs: Option<String>) -> TokenStream2 {
    match docs {
        Some(docs) => quote!(::std::option::Option::Some(#docs.to_owned())),
        None => quote!(::std::option::Option::None),
    }
}

fn adt_decl_fn(
    ast: &DeriveInput,
    type_name: &str,
    type_params: &[Ident],
) -> Result<TokenStream2, Error> {
    let type_docs = docs_expr(docs_from_attrs(&ast.attrs)?);
    let param_names: Vec<LitStr> = type_params
        .iter()
        .map(|p| LitStr::new(&p.to_string(), Span::call_site()))
        .collect();
    let adt_decl = if param_names.is_empty() {
        quote! {
            let mut __rex_supply = ::rex::typesystem::TypeVarSupply::new();
            let mut adt = ::rex::typesystem::AdtDecl::new(
                &::rex::ast::Symbol::intern(#type_name),
                &[],
                &mut __rex_supply,
            );
            adt.docs = #type_docs;
        }
    } else {
        let param_syms = param_names.iter().map(|name| {
            quote! { ::rex::ast::Symbol::intern(#name) }
        });
        quote! {
            let mut __rex_supply = ::rex::typesystem::TypeVarSupply::new();
            let mut adt = ::rex::typesystem::AdtDecl::new(
                &::rex::ast::Symbol::intern(#type_name),
                &[#(#param_syms,)*],
                &mut __rex_supply,
            );
            adt.docs = #type_docs;
        }
    };

    let mut param_bindings = Vec::new();
    let mut param_map: BTreeMap<String, TokenStream2> = BTreeMap::new();
    for p in type_params {
        let p_name = p.to_string();
        let p_lit = LitStr::new(&p_name, Span::call_site());
        let p_ident = format_ident!("__rex_param_{p_name}", span = Span::call_site());
        param_bindings.push(quote! {
            let #p_ident = adt
                .param_type(&::rex::ast::Symbol::intern(#p_lit))
                .ok_or_else(|| ::rex::typesystem::TypeError::UnknownTypeName(::rex::ast::Symbol::intern(#type_name)))?;
        });
        param_map.insert(p_name, quote!(#p_ident.clone()));
    }
    for (index, param) in ast.generics.type_params().enumerate() {
        let docs = docs_expr(docs_from_attrs(&param.attrs)?);
        param_bindings.push(quote! {
            adt.params[#index].docs = #docs;
        });
    }

    match &ast.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let ctor = type_name;
                let mut field_inits = Vec::new();
                for field in &fields.named {
                    let field_ident = field
                        .ident
                        .as_ref()
                        .ok_or_else(|| Error::new(field.span(), "expected named field"))?;
                    let mut field_name = field_ident.to_string();
                    if let Some(rename) = serde_rename_from_attrs(&field.attrs)? {
                        field_name = rename;
                    }
                    let field_ty = rex_type_expr(&field.ty, &param_map)?;
                    let field_docs = docs_expr(docs_from_attrs(&field.attrs)?);
                    field_inits.push(quote! {
                        ::rex::typesystem::AdtField {
                            name: ::rex::ast::Symbol::intern(#field_name),
                            typ: #field_ty,
                            docs: #field_docs,
                        }
                    });
                }
                let variant_docs = type_docs.clone();
                Ok(quote! {{
                    #adt_decl
                    #(#param_bindings)*
                    adt.add_variant(
                        ::rex::ast::Symbol::intern(#ctor),
                        ::std::vec![::rex::typesystem::AdtArgument::Record {
                            fields: ::std::vec![#(#field_inits,)*],
                            docs: ::std::option::Option::None,
                        }],
                        #variant_docs,
                    );
                    Ok(adt)
                }})
            }
            Fields::Unnamed(fields) => {
                let ctor = type_name;
                let mut args = Vec::new();
                for field in &fields.unnamed {
                    let ty = rex_type_expr(&field.ty, &param_map)?;
                    let docs = docs_expr(docs_from_attrs(&field.attrs)?);
                    args.push(quote! {
                        ::rex::typesystem::AdtArgument::Positional {
                            typ: #ty,
                            docs: #docs,
                        }
                    });
                }
                let variant_docs = type_docs.clone();
                Ok(quote! {{
                    #adt_decl
                    #(#param_bindings)*
                    adt.add_variant(
                        ::rex::ast::Symbol::intern(#ctor),
                        ::std::vec![#(#args,)*],
                        #variant_docs,
                    );
                    Ok(adt)
                }})
            }
            Fields::Unit => {
                let variant_docs = type_docs.clone();
                Ok(quote! {{
                    #adt_decl
                    #(#param_bindings)*
                    adt.add_variant(
                        ::rex::ast::Symbol::intern(#type_name),
                        ::std::vec![],
                        #variant_docs,
                    );
                    Ok(adt)
                }})
            }
        },
        Data::Enum(data) => {
            let mut variants = Vec::new();
            for variant in &data.variants {
                let mut variant_name = variant.ident.to_string();
                if let Some(rename) = serde_rename_from_attrs(&variant.attrs)? {
                    variant_name = rename;
                }
                let variant_docs = docs_expr(docs_from_attrs(&variant.attrs)?);
                let args = match &variant.fields {
                    Fields::Unit => Vec::new(),
                    Fields::Unnamed(fields) => {
                        let mut out = Vec::new();
                        for field in &fields.unnamed {
                            let typ = rex_type_expr(&field.ty, &param_map)?;
                            let docs = docs_expr(docs_from_attrs(&field.attrs)?);
                            out.push(quote! {
                                ::rex::typesystem::AdtArgument::Positional {
                                    typ: #typ,
                                    docs: #docs,
                                }
                            });
                        }
                        out
                    }
                    Fields::Named(fields) => {
                        let mut field_inits = Vec::new();
                        for field in &fields.named {
                            let field_ident = field
                                .ident
                                .as_ref()
                                .ok_or_else(|| Error::new(field.span(), "expected named field"))?;
                            let mut field_name = field_ident.to_string();
                            if let Some(rename) = serde_rename_from_attrs(&field.attrs)? {
                                field_name = rename;
                            }
                            let field_ty = rex_type_expr(&field.ty, &param_map)?;
                            let field_docs = docs_expr(docs_from_attrs(&field.attrs)?);
                            field_inits.push(quote! {
                                ::rex::typesystem::AdtField {
                                    name: ::rex::ast::Symbol::intern(#field_name),
                                    typ: #field_ty,
                                    docs: #field_docs,
                                }
                            });
                        }
                        let record = quote! {
                            ::rex::typesystem::AdtArgument::Record {
                                fields: ::std::vec![#(#field_inits,)*],
                                docs: ::std::option::Option::None,
                            }
                        };
                        vec![record]
                    }
                };
                variants.push(quote! {
                    adt.add_variant(
                        ::rex::ast::Symbol::intern(#variant_name),
                        ::std::vec![#(#args,)*],
                        #variant_docs,
                    );
                });
            }
            Ok(quote! {{
                #adt_decl
                #(#param_bindings)*
                #(#variants)*
                Ok(adt)
            }})
        }
        Data::Union(_) => Err(Error::new(
            ast.span(),
            "`#[derive(Rex)]` only supports structs and enums",
        )),
    }
}

fn adt_family_fn(
    ast: &DeriveInput,
    type_name: &str,
    type_params: &[Ident],
) -> Result<TokenStream2, Error> {
    let deps = collect_dependency_exprs(ast, type_name, type_params)?;
    let type_param_deps = type_params.iter().map(|param| {
        quote! {
            <#param as ::rex::typesystem::RexType>::collect_rex_family(out)?;
        }
    });
    Ok(quote! {{
        #(
            #type_param_deps
        )*
        #(
            #deps
        )*
        out.push(<Self as ::rex::typesystem::RexAdt>::rex_adt_decl()?);
        Ok(())
    }})
}

fn collect_dependency_exprs(
    ast: &DeriveInput,
    type_name: &str,
    type_params: &[Ident],
) -> Result<Vec<TokenStream2>, Error> {
    let mut deps = Vec::new();
    match &ast.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                for field in &fields.named {
                    collect_dependency_exprs_from_type(
                        &field.ty,
                        type_name,
                        type_params,
                        &mut deps,
                    )?;
                }
            }
            Fields::Unnamed(fields) => {
                for field in &fields.unnamed {
                    collect_dependency_exprs_from_type(
                        &field.ty,
                        type_name,
                        type_params,
                        &mut deps,
                    )?;
                }
            }
            Fields::Unit => {}
        },
        Data::Enum(data) => {
            for variant in &data.variants {
                match &variant.fields {
                    Fields::Named(fields) => {
                        for field in &fields.named {
                            collect_dependency_exprs_from_type(
                                &field.ty,
                                type_name,
                                type_params,
                                &mut deps,
                            )?;
                        }
                    }
                    Fields::Unnamed(fields) => {
                        for field in &fields.unnamed {
                            collect_dependency_exprs_from_type(
                                &field.ty,
                                type_name,
                                type_params,
                                &mut deps,
                            )?;
                        }
                    }
                    Fields::Unit => {}
                }
            }
        }
        Data::Union(_) => {}
    }
    Ok(dedupe_token_streams(deps))
}

fn collect_dependency_exprs_from_type(
    ty: &Type,
    self_type_name: &str,
    type_params: &[Ident],
    deps: &mut Vec<TokenStream2>,
) -> Result<(), Error> {
    match ty {
        Type::Tuple(tuple) => {
            for elem in &tuple.elems {
                collect_dependency_exprs_from_type(elem, self_type_name, type_params, deps)?;
            }
            Ok(())
        }
        Type::Path(type_path) => {
            let Some(seg) = type_path.path.segments.last() else {
                return Err(Error::new(type_path.span(), "unsupported type path"));
            };
            let ident = seg.ident.to_string();
            if type_params.iter().any(|param| param == &seg.ident)
                || ident == self_type_name
                || is_builtin_rust_type(type_path)
            {
                return Ok(());
            }

            let args = match &seg.arguments {
                PathArguments::AngleBracketed(args) => args
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(t),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };

            match ident.as_str() {
                "Vec" | "Option" => {
                    let [inner] = args.as_slice() else {
                        return Err(Error::new(seg.span(), format!("expected `{ident}<T>`")));
                    };
                    collect_dependency_exprs_from_type(inner, self_type_name, type_params, deps)
                }
                "HashMap" | "BTreeMap" => {
                    let [_key, value] = args.as_slice() else {
                        return Err(Error::new(seg.span(), format!("expected `{ident}<K, V>`")));
                    };
                    collect_dependency_exprs_from_type(value, self_type_name, type_params, deps)
                }
                "Result" => {
                    let [ok, err] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Result<T, E>`"));
                    };
                    collect_dependency_exprs_from_type(ok, self_type_name, type_params, deps)?;
                    collect_dependency_exprs_from_type(err, self_type_name, type_params, deps)
                }
                _ => {
                    deps.push(quote! { <#type_path as ::rex::typesystem::RexType>::collect_rex_family(out)?; });
                    Ok(())
                }
            }
        }
        other => Err(Error::new(
            other.span(),
            "unsupported field type for Rex dependency discovery",
        )),
    }
}

fn dedupe_token_streams(tokens: Vec<TokenStream2>) -> Vec<TokenStream2> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for token in tokens {
        let key = token.to_string();
        if seen.insert(key) {
            out.push(token);
        }
    }
    out
}

fn rex_type_expr(
    ty: &Type,
    adt_params: &BTreeMap<String, TokenStream2>,
) -> Result<TokenStream2, Error> {
    match ty {
        Type::Tuple(tuple) => {
            let elems = tuple
                .elems
                .iter()
                .map(|t| rex_type_expr(t, adt_params))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(quote! { ::rex::typesystem::Type::tuple(::std::vec![#(#elems,)*]) })
        }
        Type::Path(type_path) => {
            if type_path.qself.is_none() && type_path.path.segments.len() == 1 {
                let seg = type_path
                    .path
                    .segments
                    .last()
                    .ok_or_else(|| Error::new(type_path.span(), "unsupported type path"))?;
                let ident = seg.ident.to_string();
                if let Some(param_ty) = adt_params.get(&ident) {
                    return Ok(param_ty.clone());
                }
            }

            let seg = type_path
                .path
                .segments
                .last()
                .ok_or_else(|| Error::new(type_path.span(), "unsupported type path"))?;
            let ident = seg.ident.to_string();
            let args = match &seg.arguments {
                PathArguments::AngleBracketed(args) => args
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(t),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };

            match ident.as_str() {
                "Vec" => {
                    let [inner] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Vec<T>`"));
                    };
                    let inner = rex_type_expr(inner, adt_params)?;
                    Ok(quote! {
                        ::rex::typesystem::Type::list(#inner)
                    })
                }
                "HashMap" | "BTreeMap" => {
                    let [k, v] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `HashMap<K, V>`"));
                    };
                    if !is_string_type(k) {
                        return Err(Error::new(
                            k.span(),
                            "only `HashMap<String, V>` is supported for Rex dictionaries",
                        ));
                    }
                    let v = rex_type_expr(v, adt_params)?;
                    Ok(quote! {
                        ::rex::typesystem::Type::app(
                            ::rex::typesystem::Type::builtin(::rex::typesystem::BuiltinTypeId::Dict),
                            #v
                        )
                    })
                }
                "Option" => {
                    let [inner] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Option<T>`"));
                    };
                    let inner = rex_type_expr(inner, adt_params)?;
                    Ok(quote! {
                        ::rex::typesystem::Type::app(
                            ::rex::typesystem::Type::builtin(::rex::typesystem::BuiltinTypeId::Option),
                            #inner
                        )
                    })
                }
                "Result" => {
                    let [ok, err] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Result<T, E>`"));
                    };
                    let ok = rex_type_expr(ok, adt_params)?;
                    let err = rex_type_expr(err, adt_params)?;
                    Ok(quote! {
                        ::rex::typesystem::Type::app(
                            ::rex::typesystem::Type::app(
                                ::rex::typesystem::Type::builtin(::rex::typesystem::BuiltinTypeId::Result),
                                #err
                            ),
                            #ok
                        )
                    })
                }
                _ => Ok(quote! { <#type_path as ::rex::typesystem::RexType>::rex_type() }),
            }
        }
        other => Err(Error::new(
            other.span(),
            "unsupported field type for Rex mapping",
        )),
    }
}

fn into_value_expr(expr: TokenStream2, _ty: &Type) -> Result<TokenStream2, Error> {
    Ok(quote! { ::rex::engine::IntoRex::into_rex(#expr)? })
}

fn from_value_expr(
    value_expr: TokenStream2,
    ty: &Type,
    _name_expr: TokenStream2,
) -> Result<TokenStream2, Error> {
    Ok(quote! { <#ty as ::rex::engine::FromRex>::from_rex(#value_expr) })
}

fn is_string_type(ty: &Type) -> bool {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident == "String")
            .unwrap_or(false),
        _ => false,
    }
}

fn is_builtin_rust_type(ty: &syn::TypePath) -> bool {
    let Some(seg) = ty.path.segments.last() else {
        return false;
    };
    matches!(
        seg.ident.to_string().as_str(),
        "bool"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "f32"
            | "f64"
            | "char"
            | "String"
            | "str"
            | "Uuid"
            | "Hash"
            | "DateTime"
    )
}

fn add_bound_to_type_params(generics: &mut Generics, bound: syn::TypeParamBound) {
    for param in generics.type_params_mut() {
        param.bounds.push(bound.clone());
    }
}

fn into_value_impl(ast: &DeriveInput, type_name: &str) -> Result<TokenStream2, Error> {
    let rust_ident = &ast.ident;
    let ctor = type_name;

    let body = match &ast.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let mut inserts = Vec::new();
                for field in &fields.named {
                    let ident = field
                        .ident
                        .as_ref()
                        .ok_or_else(|| Error::new(field.span(), "expected named field"))?;
                    let mut name = ident.to_string();
                    if let Some(rename) = serde_rename_from_attrs(&field.attrs)? {
                        name = rename;
                    }
                    let enc = into_value_expr(quote!(self.#ident), &field.ty)?;
                    inserts.push(quote! {
                        map.insert(#name.to_owned(), #enc);
                    });
                }
                quote! {{
                    let mut map = ::std::collections::BTreeMap::new();
                    #(#inserts)*
                    ::rex::engine::Value::Adt(
                        ::rex::ast::Symbol::intern(#ctor),
                        ::std::vec![::rex::engine::Value::Dict(map)],
                    )
                }}
            }
            Fields::Unnamed(fields) => {
                let mut args = Vec::new();
                let mut bindings = Vec::new();
                for (idx, field) in fields.unnamed.iter().enumerate() {
                    let v = format_ident!("__rex_f{idx}", span = Span::call_site());
                    bindings.push(v.clone());
                    args.push(into_value_expr(quote!(#v), &field.ty)?);
                }
                quote! {{
                    let Self(#(#bindings,)*) = self;
                    ::rex::engine::Value::Adt(
                        ::rex::ast::Symbol::intern(#ctor),
                        ::std::vec![#(#args,)*],
                    )
                }}
            }
            Fields::Unit => quote! {
                ::rex::engine::Value::Adt(
                    ::rex::ast::Symbol::intern(#ctor),
                    ::std::vec::Vec::new(),
                )
            },
        },
        Data::Enum(data) => {
            let mut arms = Vec::new();
            for variant in &data.variants {
                let variant_ident = &variant.ident;
                let mut variant_name = variant_ident.to_string();
                if let Some(rename) = serde_rename_from_attrs(&variant.attrs)? {
                    variant_name = rename;
                }
                let arm = match &variant.fields {
                    Fields::Unit => quote! {
                        Self::#variant_ident => ::rex::engine::Value::Adt(
                            ::rex::ast::Symbol::intern(#variant_name),
                            ::std::vec::Vec::new(),
                        )
                    },
                    Fields::Unnamed(fields) => {
                        let vars: Vec<Ident> = (0..fields.unnamed.len())
                            .map(|i| format_ident!("__rex_v{i}", span = Span::call_site()))
                            .collect();
                        let encs = vars
                            .iter()
                            .zip(fields.unnamed.iter())
                            .map(|(v, f)| into_value_expr(quote!(#v), &f.ty))
                            .collect::<Result<Vec<_>, _>>()?;
                        quote! {
                            Self::#variant_ident(#(#vars,)*) => ::rex::engine::Value::Adt(
                                ::rex::ast::Symbol::intern(#variant_name),
                                ::std::vec![#(#encs,)*],
                            )
                        }
                    }
                    Fields::Named(fields) => {
                        let mut vars = Vec::new();
                        let mut inserts = Vec::new();
                        for field in &fields.named {
                            let ident = field
                                .ident
                                .as_ref()
                                .ok_or_else(|| Error::new(field.span(), "expected named field"))?;
                            vars.push(ident.clone());
                            let mut name = ident.to_string();
                            if let Some(rename) = serde_rename_from_attrs(&field.attrs)? {
                                name = rename;
                            }
                            let enc = into_value_expr(quote!(#ident), &field.ty)?;
                            inserts.push(quote! {
                                map.insert(#name.to_owned(), #enc);
                            });
                        }
                        quote! {
                            Self::#variant_ident { #(#vars,)* } => {
                                let mut map = ::std::collections::BTreeMap::new();
                                #(#inserts)*
                                ::rex::engine::Value::Adt(
                                    ::rex::ast::Symbol::intern(#variant_name),
                                    ::std::vec![::rex::engine::Value::Dict(map)],
                                )
                            }
                        }
                    }
                };
                arms.push(arm);
            }
            quote! {{
                match self {
                    #(#arms,)*
                }
            }}
        }
        Data::Union(_) => {
            return Err(Error::new(
                ast.span(),
                "`#[derive(Rex)]` only supports structs and enums",
            ));
        }
    };

    let mut generics = ast.generics.clone();
    add_bound_to_type_params(&mut generics, parse_quote!(::rex::engine::IntoRex));
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    let (_, ty_generics, _) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::rex::engine::IntoRex for #rust_ident #ty_generics #where_clause {
            fn into_rex(self) -> ::std::result::Result<::rex::engine::Value, ::rex::engine::EngineError> {
                Ok(#body)
            }
        }
    })
}

fn from_value_impl(ast: &DeriveInput, type_name: &str) -> Result<TokenStream2, Error> {
    let rust_ident = &ast.ident;
    let name_expr = quote!(name);

    let body = match &ast.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let mut field_decodes = Vec::new();
                let mut field_idents = Vec::new();
                for field in &fields.named {
                    let ident = field
                        .ident
                        .as_ref()
                        .ok_or_else(|| Error::new(field.span(), "expected named field"))?;
                    field_idents.push(ident.clone());
                    let mut name = ident.to_string();
                    if let Some(rename) = serde_rename_from_attrs(&field.attrs)? {
                        name = rename;
                    }
                    let key = quote!(#name);
                    let decode = from_value_expr(quote!(v), &field.ty, name_expr.clone())?;
                    field_decodes.push(quote! {
                        let v = map.remove(#key).ok_or_else(|| ::rex::engine::EngineError::NativeType { expected: format!("missing field `{}`", #name),
                            got: "dict".into(),
                        })?;
                        let #ident = #decode?;
                    });
                }
                Ok(quote! {{
                    match value {
                        ::rex::engine::Value::Adt(tag, mut args)
                            if tag.as_ref() == #type_name && args.len() == 1 =>
                        {
                            match args.pop().unwrap() {
                                ::rex::engine::Value::Dict(mut map) => {
                                    #(#field_decodes)*
                                    Ok(Self { #(#field_idents,)* })
                                }
                                other => Err(::rex::engine::EngineError::NativeType {
                                    expected: "dict".into(),
                                    got: other.value_type_name().into(),
                                }),
                            }
                        }
                        _ => Err(::rex::engine::EngineError::NativeType {
                            expected: #type_name.into(),
                            got: got.clone(),
                        }),
                    }
                }})
            }
            Fields::Unnamed(fields) => {
                let mut decs = Vec::new();
                for field in &fields.unnamed {
                    let decode =
                        from_value_expr(quote!(args.remove(0)), &field.ty, name_expr.clone())?;
                    decs.push(quote!(#decode?));
                }
                let len = fields.unnamed.len();
                Ok(quote! {{
                    match value {
                        ::rex::engine::Value::Adt(tag, mut args)
                            if tag.as_ref() == #type_name && args.len() == #len =>
                        {
                            Ok(Self(#(#decs,)*))
                        }
                        _ => Err(::rex::engine::EngineError::NativeType {
                            expected: #type_name.into(),
                            got: got.clone(),
                        }),
                    }
                }})
            }
            Fields::Unit => Ok(quote! {{
                match value {
                    ::rex::engine::Value::Adt(tag, args)
                        if tag.as_ref() == #type_name && args.is_empty() =>
                    {
                        Ok(Self)
                    }
                    _ => Err(::rex::engine::EngineError::NativeType {
                        expected: #type_name.into(),
                        got: got.clone(),
                    }),
                }
            }}),
        },
        Data::Enum(data) => {
            let mut arms = Vec::new();
            for variant in &data.variants {
                let variant_ident = &variant.ident;
                let mut variant_name = variant_ident.to_string();
                if let Some(rename) = serde_rename_from_attrs(&variant.attrs)? {
                    variant_name = rename;
                }
                let arm = match &variant.fields {
                    Fields::Unit => quote! {
                        ::rex::engine::Value::Adt(tag, args)
                            if tag.as_ref() == #variant_name && args.is_empty() =>
                        {
                            Ok(Self::#variant_ident)
                        }
                    },
                    Fields::Unnamed(fields) => {
                        let len = fields.unnamed.len();
                        let vals = fields
                            .unnamed
                            .iter()
                            .map(|f| {
                                from_value_expr(quote!(args.remove(0)), &f.ty, name_expr.clone())
                            })
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .map(|d| quote!(#d?))
                            .collect::<Vec<_>>();
                        quote! {
                            ::rex::engine::Value::Adt(tag, mut args)
                                if tag.as_ref() == #variant_name && args.len() == #len =>
                            {
                                Ok(Self::#variant_ident(#(#vals,)*))
                            }
                        }
                    }
                    Fields::Named(fields) => {
                        let mut field_decodes = Vec::new();
                        let mut fields_init = Vec::new();
                        for field in &fields.named {
                            let ident = field
                                .ident
                                .as_ref()
                                .ok_or_else(|| Error::new(field.span(), "expected named field"))?;
                            fields_init.push(ident.clone());
                            let mut name = ident.to_string();
                            if let Some(rename) = serde_rename_from_attrs(&field.attrs)? {
                                name = rename;
                            }
                            let key = quote!(#name);
                            let decode = from_value_expr(quote!(v), &field.ty, name_expr.clone())?;
                            field_decodes.push(quote! {
                                let v = map.remove(#key).ok_or_else(|| ::rex::engine::EngineError::NativeType { expected: format!("missing field `{}`", #name),
                                    got: "dict".into(),
                                })?;
                                let #ident = #decode?;
                            });
                        }
                        quote! {
                            ::rex::engine::Value::Adt(tag, mut args)
                                if tag.as_ref() == #variant_name && args.len() == 1 =>
                            {
                                match args.pop().unwrap() {
                                    ::rex::engine::Value::Dict(mut map) => {
                                        #(#field_decodes)*
                                        Ok(Self::#variant_ident { #(#fields_init,)* })
                                    }
                                    other => Err(::rex::engine::EngineError::NativeType {
                                        expected: "dict".into(),
                                        got: other.value_type_name().into(),
                                    }),
                                }
                            }
                        }
                    }
                };
                arms.push(arm);
            }

            Ok(quote! {{
                match value {
                    #(#arms,)*
                    _ => Err(::rex::engine::EngineError::NativeType {
                        expected: #type_name.into(),
                        got: got.clone(),
                    }),
                }
            }})
        }
        Data::Union(_) => Err(Error::new(
            ast.span(),
            "`#[derive(Rex)]` only supports structs and enums",
        )),
    }?;

    let mut generics = ast.generics.clone();
    add_bound_to_type_params(&mut generics, parse_quote!(::rex::engine::FromRex));
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    let (_, ty_generics, _) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::rex::engine::FromRex for #rust_ident #ty_generics #where_clause {
            fn from_rex(value: ::rex::engine::Value) -> Result<Self, ::rex::engine::EngineError> {
                let got = value.value_type_name().to_string();
                #body
            }
        }
    })
}
