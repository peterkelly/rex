use proc_macro::TokenStream;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Error, Fields, GenericArgument, Ident, LitStr, PathArguments,
    Type, spanned::Spanned,
};

#[proc_macro_derive(Rex, attributes(rex, serde))]
pub fn derive_rex(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();
    match expand(&ast) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

struct DeriveOptions {
    name: String,
}

fn expand(ast: &DeriveInput) -> Result<TokenStream2, Error> {
    if !ast.generics.params.is_empty() {
        return Err(Error::new(
            ast.generics.span(),
            "`#[derive(Rex)]` does not currently support generics",
        ));
    }

    let opts = DeriveOptions {
        name: rex_name_from_attrs(&ast.attrs)?.unwrap_or_else(|| ast.ident.to_string()),
    };

    let rust_ident = &ast.ident;
    let type_name = opts.name;

    let rex_type_impl = quote! {
        impl ::rex_engine::RexType for #rust_ident {
            fn rex_type() -> ::rex_ts::Type {
                ::rex_ts::Type::con(#type_name, 0)
            }
        }
    };

    let adt_decl_fn = adt_decl_fn(ast, &type_name)?;
    let inject_fn = quote! {
        impl #rust_ident {
            pub fn rex_adt_decl(engine: &mut ::rex_engine::Engine) -> ::rex_ts::AdtDecl {
                #adt_decl_fn
            }

            pub fn inject_rex(engine: &mut ::rex_engine::Engine) -> Result<(), ::rex_engine::EngineError> {
                let adt = Self::rex_adt_decl(engine);
                engine.inject_adt(adt)
            }
        }
    };

    let into_value_impl = into_value_impl(ast, &type_name)?;
    let from_value_impl = from_value_impl(ast, &type_name)?;

    Ok(quote! {
        #rex_type_impl
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
            }
            Ok(())
        })?;
        if rename.is_some() {
            return Ok(rename);
        }
    }
    Ok(None)
}

fn adt_decl_fn(ast: &DeriveInput, type_name: &str) -> Result<TokenStream2, Error> {
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
                    let field_ty = rex_type_expr(&field.ty)?;
                    field_inits.push(quote! {
                        ( ::rex_ast::expr::intern(#field_name), #field_ty )
                    });
                }
                Ok(quote! {{
                    let mut adt = engine.adt_decl(#type_name, &[]);
                    let record = ::rex_ts::Type::record(::std::vec![#(#field_inits,)*]);
                    adt.add_variant(::rex_ast::expr::intern(#ctor), ::std::vec![record]);
                    adt
                }})
            }
            Fields::Unnamed(fields) => {
                let ctor = type_name;
                let mut args = Vec::new();
                for field in &fields.unnamed {
                    let ty = rex_type_expr(&field.ty)?;
                    args.push(ty);
                }
                Ok(quote! {{
                    let mut adt = engine.adt_decl(#type_name, &[]);
                    adt.add_variant(::rex_ast::expr::intern(#ctor), ::std::vec![#(#args,)*]);
                    adt
                }})
            }
            Fields::Unit => Ok(quote! {{
                let mut adt = engine.adt_decl(#type_name, &[]);
                adt.add_variant(::rex_ast::expr::intern(#type_name), ::std::vec![]);
                adt
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
                            out.push(rex_type_expr(&field.ty)?);
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
                            let field_ty = rex_type_expr(&field.ty)?;
                            field_inits.push(quote! {
                                ( ::rex_ast::expr::intern(#field_name), #field_ty )
                            });
                        }
                        let record = quote! {
                            ::rex_ts::Type::record(::std::vec![#(#field_inits,)*])
                        };
                        vec![record]
                    }
                };
                variants.push(quote! {
                    adt.add_variant(::rex_ast::expr::intern(#variant_name), ::std::vec![#(#args,)*]);
                });
            }
            Ok(quote! {{
                let mut adt = engine.adt_decl(#type_name, &[]);
                #(#variants)*
                adt
            }})
        }
        Data::Union(_) => Err(Error::new(ast.span(), "`#[derive(Rex)]` only supports structs and enums")),
    }
}

fn rex_type_expr(ty: &Type) -> Result<TokenStream2, Error> {
    match ty {
        Type::Tuple(tuple) => {
            let elems = tuple.elems.iter().map(rex_type_expr).collect::<Result<Vec<_>, _>>()?;
            Ok(quote! { ::rex_ts::Type::tuple(::std::vec![#(#elems,)*]) })
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
                    let inner = rex_type_expr(inner)?;
                    Ok(quote! { ::rex_ts::Type::app(::rex_ts::Type::con("List", 1), #inner) })
                }
                "HashMap" | "BTreeMap" => {
                    let [_k, v] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `HashMap<K, V>`"));
                    };
                    let v = rex_type_expr(v)?;
                    Ok(quote! { ::rex_ts::Type::app(::rex_ts::Type::con("Dict", 1), #v) })
                }
                "Option" => {
                    let [inner] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Option<T>`"));
                    };
                    let inner = rex_type_expr(inner)?;
                    Ok(quote! { ::rex_ts::Type::app(::rex_ts::Type::con("Option", 1), #inner) })
                }
                "Result" => {
                    let [ok, err] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Result<T, E>`"));
                    };
                    let ok = rex_type_expr(ok)?;
                    let err = rex_type_expr(err)?;
                    Ok(quote! {
                        ::rex_ts::Type::app(
                            ::rex_ts::Type::app(::rex_ts::Type::con("Result", 2), #err),
                            #ok
                        )
                    })
                }
                _ => Ok(quote! { <#type_path as ::rex_engine::RexType>::rex_type() }),
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
                ::rex_engine::Value::Tuple(::std::vec![#(#encs,)*])
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
                        let mut out = ::rex_engine::Value::Adt(::rex_ast::expr::intern("Empty"), ::std::vec::Vec::new());
                        for item in #expr.into_iter().rev() {
                            out = ::rex_engine::Value::Adt(
                                ::rex_ast::expr::intern("Cons"),
                                ::std::vec![#inner_encode, out],
                            );
                        }
                        out
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
                            out.insert(::rex_ast::expr::intern(&k), #v_encode);
                        }
                        ::rex_engine::Value::Dict(out)
                    }})
                }
                "Option" => {
                    let [inner] = args.as_slice() else {
                        return Err(Error::new(seg.span(), "expected `Option<T>`"));
                    };
                    let inner_encode = into_value_expr(quote!(v), inner)?;
                    Ok(quote! {{
                        match #expr {
                            Some(v) => ::rex_engine::Value::Adt(::rex_ast::expr::intern("Some"), ::std::vec![#inner_encode]),
                            None => ::rex_engine::Value::Adt(::rex_ast::expr::intern("None"), ::std::vec::Vec::new()),
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
                            Ok(v) => ::rex_engine::Value::Adt(::rex_ast::expr::intern("Ok"), ::std::vec![#ok_encode]),
                            Err(e) => ::rex_engine::Value::Adt(::rex_ast::expr::intern("Err"), ::std::vec![#err_encode]),
                        }
                    }})
                }
                _ => Ok(quote! { ::rex_engine::IntoValue::into_value(#expr) }),
            }
        }
        other => Err(Error::new(
            other.span(),
            "unsupported field type for Rex encoding",
        )),
    }
}

fn from_value_expr(value_expr: TokenStream2, ty: &Type, name_expr: TokenStream2) -> Result<TokenStream2, Error> {
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
                match #value_expr {
                    ::rex_engine::Value::Tuple(items) if items.len() == #len => {
                        Ok((#(#decs?,)*))
                    }
                    other => Err(::rex_engine::EngineError::NativeType {
                        name: ::rex_ast::expr::intern(#name_expr),
                        expected: "tuple".into(),
                        got: format!("{other}"),
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
                    let inner_decode = from_value_expr(quote!(&args[0]), inner, name_expr.clone())?;
                    Ok(quote! {{
                        let mut out = ::std::vec::Vec::new();
                        let mut cur = #value_expr;
                        loop {
                            match cur {
                                ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == "Empty" && args.is_empty() => {
                                    break Ok(out);
                                }
                                ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
                                    let v = #inner_decode?;
                                    out.push(v);
                                    cur = &args[1];
                                }
                                other => {
                                    break Err(::rex_engine::EngineError::NativeType {
                                        name: ::rex_ast::expr::intern(#name_expr),
                                        expected: "list".into(),
                                        got: format!("{other}"),
                                    });
                                }
                            }
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
                        match #value_expr {
                            ::rex_engine::Value::Dict(map) => {
                                let mut out = ::std::collections::HashMap::new();
                                for (k, v) in map {
                                    let decoded = #v_decode?;
                                    out.insert(k.as_ref().to_string(), decoded);
                                }
                                Ok(out)
                            }
                            other => Err(::rex_engine::EngineError::NativeType {
                                name: ::rex_ast::expr::intern(#name_expr),
                                expected: "dict".into(),
                                got: format!("{other}"),
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
                        match #value_expr {
                            ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == "None" && args.is_empty() => Ok(None),
                            ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == "Some" && args.len() == 1 => Ok(Some(#inner_decode?)),
                            other => Err(::rex_engine::EngineError::NativeType {
                                name: ::rex_ast::expr::intern(#name_expr),
                                expected: "option".into(),
                                got: format!("{other}"),
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
                        match #value_expr {
                            ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == "Ok" && args.len() == 1 => Ok(Ok(#ok_decode?)),
                            ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == "Err" && args.len() == 1 => Ok(Err(#err_decode?)),
                            other => Err(::rex_engine::EngineError::NativeType {
                                name: ::rex_ast::expr::intern(#name_expr),
                                expected: "result".into(),
                                got: format!("{other}"),
                            }),
                        }
                    }})
                }
                _ => Ok(quote! { <#type_path as ::rex_engine::FromValue>::from_value(#value_expr, #name_expr) }),
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
                        map.insert(::rex_ast::expr::intern(#name), #enc);
                    });
                }
                quote! {{
                    let mut map = ::std::collections::BTreeMap::new();
                    #(#inserts)*
                    ::rex_engine::Value::Adt(::rex_ast::expr::intern(#ctor), ::std::vec![::rex_engine::Value::Dict(map)])
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
                    ::rex_engine::Value::Adt(::rex_ast::expr::intern(#ctor), ::std::vec![#(#args,)*])
                }}
            }
            Fields::Unit => quote! {
                ::rex_engine::Value::Adt(::rex_ast::expr::intern(#ctor), ::std::vec::Vec::new())
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
                        Self::#variant_ident => ::rex_engine::Value::Adt(::rex_ast::expr::intern(#variant_name), ::std::vec::Vec::new())
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
                            Self::#variant_ident(#(#vars,)*) => ::rex_engine::Value::Adt(::rex_ast::expr::intern(#variant_name), ::std::vec![#(#encs,)*])
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
                                map.insert(::rex_ast::expr::intern(#name), #enc);
                            });
                        }
                        quote! {
                            Self::#variant_ident { #(#vars,)* } => {
                                let mut map = ::std::collections::BTreeMap::new();
                                #(#inserts)*
                                ::rex_engine::Value::Adt(::rex_ast::expr::intern(#variant_name), ::std::vec![::rex_engine::Value::Dict(map)])
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
        Data::Union(_) => return Err(Error::new(ast.span(), "`#[derive(Rex)]` only supports structs and enums")),
    };

    Ok(quote! {
        impl ::rex_engine::IntoValue for #rust_ident {
            fn into_value(self) -> ::rex_engine::Value {
                #body
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
                for field in &fields.named {
                    let ident = field
                        .ident
                        .as_ref()
                        .ok_or_else(|| Error::new(field.span(), "expected named field"))?;
                    let mut name = ident.to_string();
                    if let Some(rename) = serde_rename_from_attrs(&field.attrs)? {
                        name = rename;
                    }
                    let key = quote!(::rex_ast::expr::intern(#name));
                    let decode = from_value_expr(quote!(v), &field.ty, name_expr.clone())?;
                    field_decodes.push(quote! {
                        let v = map.get(&#key).ok_or_else(|| ::rex_engine::EngineError::NativeType {
                            name: ::rex_ast::expr::intern(name),
                            expected: format!("missing field `{}`", #name),
                            got: "dict".into(),
                        })?;
                        let #ident = #decode?;
                    });
                }
                let fields_init = fields.named.iter().map(|f| f.ident.as_ref().unwrap());
                Ok(quote! {{
                    match value {
                        ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == #type_name && args.len() == 1 => {
                            match &args[0] {
                                ::rex_engine::Value::Dict(map) => {
                                    #(#field_decodes)*
                                    Ok(Self { #(#fields_init,)* })
                                }
                                other => Err(::rex_engine::EngineError::NativeType {
                                    name: ::rex_ast::expr::intern(name),
                                    expected: "dict".into(),
                                    got: format!("{other}"),
                                }),
                            }
                        }
                        other => Err(::rex_engine::EngineError::NativeType {
                            name: ::rex_ast::expr::intern(name),
                            expected: #type_name.into(),
                            got: format!("{other}"),
                        }),
                    }
                }})
            }
            Fields::Unnamed(fields) => {
                let mut decs = Vec::new();
                for (idx, field) in fields.unnamed.iter().enumerate() {
                    let decode = from_value_expr(quote!(&args[#idx]), &field.ty, name_expr.clone())?;
                    decs.push(quote!(#decode?));
                }
                let len = fields.unnamed.len();
                Ok(quote! {{
                    match value {
                        ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == #type_name && args.len() == #len => {
                            Ok(Self(#(#decs,)*))
                        }
                        other => Err(::rex_engine::EngineError::NativeType {
                            name: ::rex_ast::expr::intern(name),
                            expected: #type_name.into(),
                            got: format!("{other}"),
                        }),
                    }
                }})
            }
            Fields::Unit => Ok(quote! {{
                match value {
                    ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == #type_name && args.is_empty() => Ok(Self),
                    other => Err(::rex_engine::EngineError::NativeType {
                        name: ::rex_ast::expr::intern(name),
                        expected: #type_name.into(),
                        got: format!("{other}"),
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
                        ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == #variant_name && args.is_empty() => Ok(Self::#variant_ident)
                    },
                    Fields::Unnamed(fields) => {
                        let len = fields.unnamed.len();
                        let vals = fields
                            .unnamed
                            .iter()
                            .enumerate()
                            .map(|(i, f)| from_value_expr(quote!(&args[#i]), &f.ty, name_expr.clone()))
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .map(|d| quote!(#d?))
                            .collect::<Vec<_>>();
                        quote! {
                            ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == #variant_name && args.len() == #len => {
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
                            let key = quote!(::rex_ast::expr::intern(#name));
                            let decode = from_value_expr(quote!(v), &field.ty, name_expr.clone())?;
                            field_decodes.push(quote! {
                                let v = map.get(&#key).ok_or_else(|| ::rex_engine::EngineError::NativeType {
                                    name: ::rex_ast::expr::intern(name),
                                    expected: format!("missing field `{}`", #name),
                                    got: "dict".into(),
                                })?;
                                let #ident = #decode?;
                            });
                        }
                        quote! {
                            ::rex_engine::Value::Adt(tag, args) if tag.as_ref() == #variant_name && args.len() == 1 => {
                                match &args[0] {
                                    ::rex_engine::Value::Dict(map) => {
                                        #(#field_decodes)*
                                        Ok(Self::#variant_ident { #(#fields_init,)* })
                                    }
                                    other => Err(::rex_engine::EngineError::NativeType {
                                        name: ::rex_ast::expr::intern(name),
                                        expected: "dict".into(),
                                        got: format!("{other}"),
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
                    other => Err(::rex_engine::EngineError::NativeType {
                        name: ::rex_ast::expr::intern(name),
                        expected: #type_name.into(),
                        got: format!("{other}"),
                    }),
                }
            }})
        }
        Data::Union(_) => Err(Error::new(
            ast.span(),
            "`#[derive(Rex)]` only supports structs and enums",
        )),
    }?;

    Ok(quote! {
        impl ::rex_engine::FromValue for #rust_ident {
            fn from_value(value: &::rex_engine::Value, name: &str) -> Result<Self, ::rex_engine::EngineError> {
                #body
            }
        }
    })
}
