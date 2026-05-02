#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use proc_macro::TokenStream;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use std::collections::BTreeMap;
use syn::{
    Attribute, Data, DeriveInput, Error, Fields, GenericArgument, Generics, Ident, LitStr,
    PathArguments, Type, parse_quote, spanned::Spanned,
};

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
    add_bound_to_type_params(&mut rex_type_generics, parse_quote!(::rex::engine::RexType));
    let (rex_type_impl_generics, rex_type_ty_generics, rex_type_where_clause) =
        rex_type_generics.split_for_impl();
    let rex_type_params = type_param_idents.iter().map(|ident| {
        quote! { <#ident as ::rex::engine::RexType>::rex_type() }
    });
    let rex_type_collect_family = adt_family_fn(ast, &type_name, &type_param_idents)?;
    let rex_type_impl = quote! {
        impl #rex_type_impl_generics ::rex::engine::RexType for #rust_ident #rex_type_ty_generics #rex_type_where_clause {
            fn rex_type() -> ::rex::typesystem::Type {
                let mut ty = ::rex::typesystem::Type::con(#type_name, #type_param_count);
                #( ty = ::rex::typesystem::Type::app(ty, #rex_type_params); )*
                ty
            }

            fn collect_rex_family(
                out: &mut ::std::vec::Vec<::rex::typesystem::AdtDecl>,
            ) -> Result<(), ::rex::engine::EngineError> {
                #rex_type_collect_family
            }
        }
    };
    let adt_decl_fn = adt_decl_fn(ast, &type_name, &type_param_idents)?;
    let mut rex_adt_generics = ast.generics.clone();
    add_bound_to_type_params(&mut rex_adt_generics, parse_quote!(::rex::engine::RexType));
    let (rex_adt_impl_generics, rex_adt_ty_generics, rex_adt_where_clause) =
        rex_adt_generics.split_for_impl();
    let rex_adt_impl = quote! {
        impl #rex_adt_impl_generics ::rex::engine::RexAdt for #rust_ident #rex_adt_ty_generics #rex_adt_where_clause {
            fn rex_adt_decl() -> Result<::rex::typesystem::AdtDecl, ::rex::engine::EngineError> {
                #adt_decl_fn
            }
        }
    };
    let inject_fn = quote! {
        impl #rex_adt_impl_generics #rust_ident #rex_adt_ty_generics #rex_adt_where_clause {
            pub fn inject_rex<State: Clone + Send + Sync + 'static>(
                engine: &mut ::rex::engine::Engine<State>,
            ) -> Result<(), ::rex::engine::EngineError> {
                <Self as ::rex::engine::RexAdt>::inject_rex(engine)
            }

            pub fn rex_adt_decl() -> Result<::rex::typesystem::AdtDecl, ::rex::engine::EngineError> {
                <Self as ::rex::engine::RexAdt>::rex_adt_decl()
            }

            pub fn rex_adt_family() -> Result<::std::vec::Vec<::rex::typesystem::AdtDecl>, ::rex::engine::EngineError> {
                <Self as ::rex::engine::RexAdt>::rex_adt_family()
            }

            pub fn inject_rex_with_default<State: Clone + Send + Sync + 'static>(
                engine: &mut ::rex::engine::Engine<State>,
            ) -> Result<(), ::rex::engine::EngineError>
            where
                Self: ::rex::engine::RexDefault<State>,
            {
                <Self as ::rex::engine::RexAdt>::inject_rex(engine)?;
                engine.inject_rex_default_instance::<Self>()
            }

            pub fn inject_rex_with_constructor<State, Sig, H>(
                engine: &mut ::rex::engine::Engine<State>,
                constructor: H,
            ) -> Result<(), ::rex::engine::EngineError>
            where
                State: Clone + Send + Sync + 'static,
                H: ::rex::engine::HostFnSync<State, Sig>,
            {
                <Self as ::rex::engine::RexAdt>::inject_rex(engine)?;
                let mut module = ::rex::engine::Module::global();
                module.export(#type_name, constructor)?;
                engine.inject_module(module)
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
    for attr in attrs {
        if !attr.path().is_ident("rex") {
            continue;
        }
        let mut name: Option<String> = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                name = Some(lit.value());
            }
            Ok(())
        })?;
        return Ok(name);
    }
    Ok(None)
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

fn adt_decl_fn(
    ast: &DeriveInput,
    type_name: &str,
    type_params: &[Ident],
) -> Result<TokenStream2, Error> {
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
                .ok_or_else(|| ::rex::engine::EngineError::UnknownType(::rex::ast::Symbol::intern(#type_name)))?;
        });
        param_map.insert(p_name, quote!(#p_ident.clone()));
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
                    field_inits.push(quote! {
                        ( ::rex::ast::Symbol::intern(#field_name), #field_ty )
                    });
                }
                Ok(quote! {{
                    #adt_decl
                    #(#param_bindings)*
                    let record = ::rex::typesystem::Type::record(::std::vec![#(#field_inits,)*]);
                    adt.add_variant(::rex::ast::Symbol::intern(#ctor), ::std::vec![record]);
                    Ok(adt)
                }})
            }
            Fields::Unnamed(fields) => {
                let ctor = type_name;
                let mut args = Vec::new();
                for field in &fields.unnamed {
                    let ty = rex_type_expr(&field.ty, &param_map)?;
                    args.push(ty);
                }
                Ok(quote! {{
                    #adt_decl
                    #(#param_bindings)*
                    adt.add_variant(::rex::ast::Symbol::intern(#ctor), ::std::vec![#(#args,)*]);
                    Ok(adt)
                }})
            }
            Fields::Unit => Ok(quote! {{
                #adt_decl
                #(#param_bindings)*
                adt.add_variant(::rex::ast::Symbol::intern(#type_name), ::std::vec![]);
                Ok(adt)
            }}),
        },
        Data::Enum(data) => {
            let mut variants = Vec::new();
            for variant in &data.variants {
                let mut variant_name = variant.ident.to_string();
                if let Some(rename) = serde_rename_from_attrs(&variant.attrs)? {
                    variant_name = rename;
                }
                let args = match &variant.fields {
                    Fields::Unit => Vec::new(),
                    Fields::Unnamed(fields) => {
                        let mut out = Vec::new();
                        for field in &fields.unnamed {
                            out.push(rex_type_expr(&field.ty, &param_map)?);
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
                            field_inits.push(quote! {
                                ( ::rex::ast::Symbol::intern(#field_name), #field_ty )
                            });
                        }
                        let record = quote! {
                            ::rex::typesystem::Type::record(::std::vec![#(#field_inits,)*])
                        };
                        vec![record]
                    }
                };
                variants.push(quote! {
                    adt.add_variant(::rex::ast::Symbol::intern(#variant_name), ::std::vec![#(#args,)*]);
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
    Ok(quote! {{
        #(
            #deps
        )*
        out.push(<Self as ::rex::engine::RexAdt>::rex_adt_decl()?);
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
                    deps.push(quote! { <#type_path as ::rex::engine::RexType>::collect_rex_family(out)?; });
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
                        ::rex::typesystem::Type::array(#inner)
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
                _ => Ok(quote! { <#type_path as ::rex::engine::RexType>::rex_type() }),
            }
        }
        other => Err(Error::new(
            other.span(),
            "unsupported field type for Rex mapping",
        )),
    }
}

fn into_value_expr(expr: TokenStream2, ty: &Type) -> Result<TokenStream2, Error> {
    match ty {
        Type::Tuple(tuple) => {
            let vars: Vec<Ident> = (0..tuple.elems.len())
                .map(|i| format_ident!("__rex_t{i}", span = Span::call_site()))
                .collect();
            let encs = vars
                .iter()
                .zip(tuple.elems.iter())
                .map(|(v, t)| into_value_expr(quote!(#v), t))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(quote! {{
                let (#(#vars,)*) = #expr;
                heap.alloc_tuple(::std::vec![#(#encs,)*])?
            }})
        }
        Type::Path(type_path) => {
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
                    let inner_encode = into_value_expr(quote!(item), inner)?;
                    Ok(quote! {{
                        let mut out = ::std::vec::Vec::new();
                        for item in #expr.into_iter() {
                            out.push(#inner_encode);
                        }
                        heap.alloc_array(out)?
                    }})
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
                    let v_encode = into_value_expr(quote!(v), v)?;
                    Ok(quote! {{
                        let mut out = ::std::collections::BTreeMap::new();
                        for (k, v) in #expr {
                            out.insert(::rex::ast::Symbol::intern(&k), #v_encode);
                        }
                        heap.alloc_dict(out)?
                    }})
                }
                "Option" => {
                    let [inner] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Option<T>`"));
                    };
                    let inner_encode = into_value_expr(quote!(v), inner)?;
                    Ok(quote! {{
                        match #expr {
                            Some(v) => heap
                                .alloc_adt(::rex::ast::Symbol::intern("Some"), ::std::vec![#inner_encode])?,
                            None => heap
                                .alloc_adt(::rex::ast::Symbol::intern("None"), ::std::vec::Vec::new())?,
                        }
                    }})
                }
                "Result" => {
                    let [ok_ty, err_ty] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Result<T, E>`"));
                    };
                    let ok_encode = into_value_expr(quote!(v), ok_ty)?;
                    let err_encode = into_value_expr(quote!(e), err_ty)?;
                    Ok(quote! {{
                        match #expr {
                            Ok(v) => heap
                                .alloc_adt(::rex::ast::Symbol::intern("Ok"), ::std::vec![#ok_encode])?,
                            Err(e) => heap
                                .alloc_adt(::rex::ast::Symbol::intern("Err"), ::std::vec![#err_encode])?,
                        }
                    }})
                }
                _ => Ok(quote! { ::rex::engine::IntoRex::into_rex(#expr, heap)? }),
            }
        }
        other => Err(Error::new(
            other.span(),
            "unsupported field type for Rex encoding",
        )),
    }
}

fn from_value_expr(
    handle_expr: TokenStream2,
    ty: &Type,
    name_expr: TokenStream2,
) -> Result<TokenStream2, Error> {
    match ty {
        Type::Tuple(tuple) => {
            let elem_tys = tuple.elems.iter().collect::<Vec<_>>();
            let indices: Vec<usize> = (0..elem_tys.len()).collect();
            let decs = elem_tys
                .iter()
                .zip(indices.iter())
                .map(|(t, i)| from_value_expr(quote!(&items[#i]), t, name_expr.clone()))
                .collect::<Result<Vec<_>, _>>()?;
            let len = elem_tys.len();
            Ok(quote! {{
                let __rex_handle = #handle_expr;
                match __rex_handle.value()? {
                    ::rex::engine::Value::Tuple(items) if items.len() == #len => {
                        Ok((#(#decs?,)*))
                    }
                    _ => Err(::rex::engine::EngineError::NativeType {
                        expected: "tuple".into(),
                        got: __rex_handle.type_name()?.into(),
                    }),
                }
            }})
        }
        Type::Path(type_path) => {
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
                    let inner_decode = from_value_expr(quote!(item), inner, name_expr.clone())?;
                    Ok(quote! {{
                        let __rex_handle = #handle_expr;
                        match __rex_handle.value()? {
                            ::rex::engine::Value::Array(items) => {
                                let mut out = ::std::vec::Vec::with_capacity(items.len());
                                for item in &items {
                                    let v = #inner_decode?;
                                    out.push(v);
                                }
                                Ok(out)
                            }
                            _ => Err(::rex::engine::EngineError::NativeType {
                                expected: "array".into(),
                                got: __rex_handle.type_name()?.into(),
                            }),
                        }
                    }})
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
                    let v_decode = from_value_expr(quote!(v), v, name_expr.clone())?;
                    Ok(quote! {{
                        let __rex_handle = #handle_expr;
                        match __rex_handle.value()? {
                            ::rex::engine::Value::Dict(map) => {
                                let mut out: #type_path = ::std::default::Default::default();
                                for (k, v) in &map {
                                    let decoded = #v_decode?;
                                    out.insert(k.as_ref().to_string(), decoded);
                                }
                                Ok(out)
                            }
                            _ => Err(::rex::engine::EngineError::NativeType {
                                expected: "dict".into(),
                                got: __rex_handle.type_name()?.into(),
                            }),
                        }
                    }})
                }
                "Option" => {
                    let [inner] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Option<T>`"));
                    };
                    let inner_decode = from_value_expr(quote!(&args[0]), inner, name_expr.clone())?;
                    Ok(quote! {{
                        let __rex_handle = #handle_expr;
                        match __rex_handle.value()? {
                            ::rex::engine::Value::Adt(tag, args) if tag.as_ref() == "None" && args.is_empty() => Ok(None),
                            ::rex::engine::Value::Adt(tag, args) if tag.as_ref() == "Some" && args.len() == 1 => Ok(Some(#inner_decode?)),
                            _ => Err(::rex::engine::EngineError::NativeType {
                                expected: "option".into(),
                                got: __rex_handle.type_name()?.into(),
                            }),
                        }
                    }})
                }
                "Result" => {
                    let [ok_ty, err_ty] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Result<T, E>`"));
                    };
                    let ok_decode = from_value_expr(quote!(&args[0]), ok_ty, name_expr.clone())?;
                    let err_decode = from_value_expr(quote!(&args[0]), err_ty, name_expr.clone())?;
                    Ok(quote! {{
                        let __rex_handle = #handle_expr;
                        match __rex_handle.value()? {
                            ::rex::engine::Value::Adt(tag, args) if tag.as_ref() == "Ok" && args.len() == 1 => Ok(Ok(#ok_decode?)),
                            ::rex::engine::Value::Adt(tag, args) if tag.as_ref() == "Err" && args.len() == 1 => Ok(Err(#err_decode?)),
                            _ => Err(::rex::engine::EngineError::NativeType {
                                expected: "result".into(),
                                got: __rex_handle.type_name()?.into(),
                            }),
                        }
                    }})
                }
                _ => Ok(quote! { <#type_path as ::rex::engine::FromRex>::from_rex(#handle_expr) }),
            }
        }
        other => Err(Error::new(
            other.span(),
            "unsupported field type for Rex decoding",
        )),
    }
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
            | "String"
            | "str"
            | "Uuid"
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
                        map.insert(::rex::ast::Symbol::intern(#name), #enc);
                    });
                }
                quote! {{
                    let mut map = ::std::collections::BTreeMap::new();
                    #(#inserts)*
                    let dict = heap.alloc_dict(map)?;
                    heap.alloc_adt(::rex::ast::Symbol::intern(#ctor), ::std::vec![dict])?
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
                    heap.alloc_adt(::rex::ast::Symbol::intern(#ctor), ::std::vec![#(#args,)*])?
                }}
            }
            Fields::Unit => quote! {
                heap.alloc_adt(::rex::ast::Symbol::intern(#ctor), ::std::vec::Vec::new())?
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
                        Self::#variant_ident => heap
                            .alloc_adt(::rex::ast::Symbol::intern(#variant_name), ::std::vec::Vec::new())?
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
                            Self::#variant_ident(#(#vars,)*) => heap
                                .alloc_adt(::rex::ast::Symbol::intern(#variant_name), ::std::vec![#(#encs,)*])?
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
                                map.insert(::rex::ast::Symbol::intern(#name), #enc);
                            });
                        }
                        quote! {
                            Self::#variant_ident { #(#vars,)* } => {
                                let mut map = ::std::collections::BTreeMap::new();
                                #(#inserts)*
                                let dict = heap.alloc_dict(map)?;
                                heap.alloc_adt(::rex::ast::Symbol::intern(#variant_name), ::std::vec![dict])?
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
            fn into_rex(
                self,
                heap: &::rex::engine::Heap,
            ) -> ::std::result::Result<::rex::engine::Handle, ::rex::engine::EngineError> {
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
                    let key = quote!(::rex::ast::Symbol::intern(#name));
                    let decode = from_value_expr(quote!(v), &field.ty, name_expr.clone())?;
                    field_decodes.push(quote! {
                        let v = map.get(&#key).ok_or_else(|| ::rex::engine::EngineError::NativeType { expected: format!("missing field `{}`", #name),
                            got: "dict".into(),
                        })?;
                        let #ident = #decode?;
                    });
                }
                Ok(quote! {{
                    match value {
                        ::rex::engine::Value::Adt(tag, args)
                            if (tag.as_ref() == #type_name
                                || tag.as_ref().rsplit('.').next() == Some(#type_name))
                                && args.len() == 1 =>
                        {
                            match args[0].value()? {
                                ::rex::engine::Value::Dict(map) => {
                                    #(#field_decodes)*
                                    Ok(Self { #(#field_idents,)* })
                                }
                                _ => Err(::rex::engine::EngineError::NativeType {
                                    expected: "dict".into(),
                                    got: args[0].type_name()?.into(),
                                }),
                            }
                        }
                        _ => Err(::rex::engine::EngineError::NativeType {
                            expected: #type_name.into(),
                            got: handle.type_name()?.into(),
                        }),
                    }
                }})
            }
            Fields::Unnamed(fields) => {
                let mut decs = Vec::new();
                for (idx, field) in fields.unnamed.iter().enumerate() {
                    let decode =
                        from_value_expr(quote!(&args[#idx]), &field.ty, name_expr.clone())?;
                    decs.push(quote!(#decode?));
                }
                let len = fields.unnamed.len();
                Ok(quote! {{
                    match value {
                        ::rex::engine::Value::Adt(tag, args)
                            if (tag.as_ref() == #type_name
                                || tag.as_ref().rsplit('.').next() == Some(#type_name))
                                && args.len() == #len =>
                        {
                            Ok(Self(#(#decs,)*))
                        }
                        _ => Err(::rex::engine::EngineError::NativeType {
                            expected: #type_name.into(),
                            got: handle.type_name()?.into(),
                        }),
                    }
                }})
            }
            Fields::Unit => Ok(quote! {{
                match value {
                    ::rex::engine::Value::Adt(tag, args)
                        if (tag.as_ref() == #type_name
                            || tag.as_ref().rsplit('.').next() == Some(#type_name))
                            && args.is_empty() =>
                    {
                        Ok(Self)
                    }
                    _ => Err(::rex::engine::EngineError::NativeType {
                        expected: #type_name.into(),
                        got: handle.type_name()?.into(),
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
                            if (tag.as_ref() == #variant_name
                                || tag.as_ref().rsplit('.').next() == Some(#variant_name))
                                && args.is_empty() =>
                        {
                            Ok(Self::#variant_ident)
                        }
                    },
                    Fields::Unnamed(fields) => {
                        let len = fields.unnamed.len();
                        let vals = fields
                            .unnamed
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                from_value_expr(quote!(&args[#i]), &f.ty, name_expr.clone())
                            })
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .map(|d| quote!(#d?))
                            .collect::<Vec<_>>();
                        quote! {
                            ::rex::engine::Value::Adt(tag, args)
                                if (tag.as_ref() == #variant_name
                                    || tag.as_ref().rsplit('.').next() == Some(#variant_name))
                                    && args.len() == #len =>
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
                            let key = quote!(::rex::ast::Symbol::intern(#name));
                            let decode = from_value_expr(quote!(v), &field.ty, name_expr.clone())?;
                            field_decodes.push(quote! {
                                let v = map.get(&#key).ok_or_else(|| ::rex::engine::EngineError::NativeType { expected: format!("missing field `{}`", #name),
                                    got: "dict".into(),
                                })?;
                                let #ident = #decode?;
                            });
                        }
                        quote! {
                            ::rex::engine::Value::Adt(tag, args)
                                if (tag.as_ref() == #variant_name
                                    || tag.as_ref().rsplit('.').next() == Some(#variant_name))
                                    && args.len() == 1 =>
                            {
                                match args[0].value()? {
                                    ::rex::engine::Value::Dict(map) => {
                                        #(#field_decodes)*
                                        Ok(Self::#variant_ident { #(#fields_init,)* })
                                    }
                                    _ => Err(::rex::engine::EngineError::NativeType {
                                        expected: "dict".into(),
                                        got: args[0].type_name()?.into(),
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
                        got: handle.type_name()?.into(),
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
            fn from_rex(handle: &::rex::engine::Handle) -> Result<Self, ::rex::engine::EngineError> {
                let value = handle.value()?;
                #body
            }
        }
    })
}
