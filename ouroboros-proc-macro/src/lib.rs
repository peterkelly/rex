use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, FnArg, ItemFn, ReturnType, Type, Variant};

#[proc_macro_derive(TypeInfo)]
pub fn derive_type_info(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();
    let name = &ast.ident;
    let name_as_str = format!("{name}");

    let description = match &ast.data {
        // Struct
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => {
                let fields = named.named.iter().map(|field| {
                    let field_docs = field
                        .attrs
                        .iter()
                        .filter_map(|attr| {
                            if attr.path().is_ident("doc") {
                                Some(attr.meta.clone())
                            } else {
                                None
                            }
                        })
                        .filter_map(|meta| {
                            if let syn::Meta::NameValue(value) = meta {
                                if let syn::Expr::Lit(syn::ExprLit {
                                    lit: syn::Lit::Str(lit_str),
                                    ..
                                }) = value.value
                                {
                                    let doc_str = lit_str.token().to_string();
                                    if doc_str.starts_with("\" ") && doc_str.ends_with('\"') {
                                        Some(doc_str[2..doc_str.len() - 1].to_string())
                                    } else if doc_str.starts_with('\"') && doc_str.ends_with('\"') {
                                        Some(doc_str[1..doc_str.len() - 1].to_string())
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    let field_name = format!("{}", field.ident.clone().unwrap());
                    let field_type = describe_type(&field.ty);
                    if field_docs.is_empty() {
                        quote! {
                            fields.push(::ouroboros::NamedField::new(#field_name, #field_type));
                        }
                    } else {
                        let field_doc = field_docs.join("\n");
                        quote! {
                            fields.push(::ouroboros::NamedField::new(#field_name, #field_type));
                            field_docs.insert(#field_name.to_string(), #field_doc.to_string());
                        }
                    }
                });
                quote!(
                    let mut fields = ::std::vec::Vec::new();
                    let mut field_docs = ::std::collections::HashMap::new();
                    #(#fields;)*
                    ::ouroboros::Type::Record(::ouroboros::Record::with_doc(::ouroboros::RecordDocs::named(None, field_docs), #name_as_str, fields))
                )
            }
            Fields::Unnamed(unnamed) => {
                let fields = unnamed.unnamed.iter().map(|field| {
                    let field_type = describe_type(&field.ty);
                    quote! {
                        #field_type
                    }
                });
                quote!(::ouroboros::Type::Record(::ouroboros::Record::new(#name_as_str, [#(#fields,)*].map(::ouroboros::UnnamedField::new))))
            }
            Fields::Unit => quote!(
                ::ouroboros::Type::Record(::ouroboros::Record::new_unit(#name_as_str))
            ),
        },

        // Enum
        Data::Enum(data) => {
            let is_enum = data.variants.iter().all(is_enum_variant);

            if is_enum {
                let variants = data.variants.iter().map(|variant| {
                    let mut variant_name = format!("{}", variant.ident.clone());
                    rename_variant(&mut variant_name, variant);

                    let discriminant =
                        if let Some((_, syn::Expr::Lit(ref literal))) = variant.discriminant {
                            if let syn::Lit::Int(i) = &literal.lit {
                                Some(i)
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                    if let Some(i) = discriminant {
                        quote! {
                            ::ouroboros::EnumVariant::with_const_value(#variant_name, #i)
                        }
                    } else {
                        quote! {
                            ::ouroboros::EnumVariant::new(#variant_name)
                        }
                    }
                });
                quote!(::ouroboros::Type::Enum(::ouroboros::Enum::new(#name_as_str, [#(#variants,)*])))
            } else {
                let variants = data.variants.iter().map(|variant| {
                    let mut variant_name = format!("{}", variant.ident.clone());
                    rename_variant(&mut variant_name, variant);

                    match &variant.fields {
                        Fields::Unnamed(unnamed) => {
                            let fields = unnamed.unnamed.iter().map(|field| {
                                let field_type = describe_type(&field.ty);
                                quote! {
                                    #field_type
                                }
                            });
                            quote! {
                                ::ouroboros::UnionVariant::with_fields(#variant_name, [#(#fields,)*])
                            }
                        }
                        Fields::Named(named) => {
                            let fields = named.named.iter().map(|field| {
                                let field_name = format!("{}", field.ident.clone().unwrap());
                                let field_type = describe_type(&field.ty);
                                quote! {
                                    (#field_name, #field_type)
                                }
                            });
                            quote! {
                                ::ouroboros::UnionVariant::with_fields(#variant_name, [#(#fields,)*])
                            }
                        }
                        Fields::Unit => quote!{ ::ouroboros::UnionVariant::new(#variant_name) },
                    }
                });
                quote!(::ouroboros::Type::Union(::ouroboros::Union::new(#name_as_str, [#(#variants,)*])))
            }
        }
        _ => panic!("TypeInfo can only be derived for structs and enums"),
    };

    let generics = ast.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let generic_type_names = generics
        .params
        .iter()
        .map(|param| match param {
            syn::GenericParam::Type(type_param) => {
                let ident = &type_param.ident;
                quote! { #ident::tname() }
            }
            _ => panic!("Generic type parameters are the only supported generics"),
        })
        .collect::<Vec<_>>();

    let expanded = quote! {
        impl  #impl_generics ::ouroboros::TypeInfo for #name #ty_generics #where_clause {
            fn tname() -> ::ouroboros::TypeName {
                ::ouroboros::TypeName { n: #name_as_str, g: vec![#(#generic_type_names,)*] }
            }

            fn t() -> ::ouroboros::Type {
                #description
            }
        }
    };

    TokenStream::from(expanded)
}

fn describe_type(ty: &Type) -> proc_macro2::TokenStream {
    match ty {
        Type::Path(type_path) if type_path.qself.is_none() => {
            let ident = &type_path.path.segments.last().unwrap().ident;
            let inner_types = &type_path.path.segments.last().unwrap().arguments;
            quote!(<#ident #inner_types as ::ouroboros::TypeInfo>::t())
        }
        Type::Tuple(tuple) => {
            let inner_types = tuple.elems.iter().map(describe_type);
            quote!(::ouroboros::Type::Tuple(::ouroboros::Tuple([#(#inner_types,)*].into_iter().collect())))
        }
        _ => panic!("Unsupported type"),
    }
}

fn is_enum_variant(variant: &Variant) -> bool {
    matches!(variant.fields, Fields::Unit)
}

#[proc_macro_attribute]
pub fn entrypoint(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let fn_name = &input_fn.sig.ident;
    let entrypoint_fn_name = format_ident!("__entrypoint__{}", fn_name);
    let inputs = &input_fn.sig.inputs;
    let output_type = &input_fn.sig.output;

    // Extracting input types (assuming a single tuple argument)
    let input_types = if let Some(FnArg::Typed(pat_type)) = inputs.first() {
        &pat_type.ty
    } else {
        panic!("Expected function with a single tuple argument");
    };

    // Extracting output type
    let output_type = if let ReturnType::Type(_, ty) = output_type {
        ty
    } else {
        panic!("Expected function with a return type");
    };

    // Create the original function unchanged
    let original_fn = quote! { #input_fn };

    // Create the __ouroboros__entrypoint function
    let entrypoint_fn = quote! {
        #[no_mangle]
        extern "C" fn #entrypoint_fn_name(args: *const std::os::raw::c_char) -> *mut std::os::raw::c_char {
            let args = unsafe { ::ouroboros_wasm::decode_args::<#input_types, #output_type>(stringify!(#fn_name), args) };
            match args {
                ::ouroboros_wasm::ParseResult::Args(args) => ::ouroboros_wasm::encode_result(
                    #fn_name(args)
                ),
                ::ouroboros_wasm::ParseResult::Manifest(manifest) => ::ouroboros_wasm::encode_result_pretty(manifest),
            }
        }
    };

    // Combine both functions into the final output
    let output = quote! {
        #original_fn
        #entrypoint_fn
    };

    output.into()
}

fn rename_variant(variant_name: &mut String, variant: &syn::Variant) {
    for attr in &variant.attrs {
        let maybe_meta_tokens_iter = attr.meta.require_list().ok().and_then(|meta_list| {
            meta_list
                .path
                .segments
                .first()
                .filter(|serde_ident| serde_ident.ident == "serde")
                .map(|_| meta_list.tokens.clone().into_iter())
        });
        if let Some(mut meta_tokens_iter) = maybe_meta_tokens_iter {
            match (
                meta_tokens_iter.next().map(|t| t.to_string()).as_deref(),
                meta_tokens_iter.next().map(|t| t.to_string()).as_deref(),
                meta_tokens_iter.next().map(|t| t.to_string()),
            ) {
                (Some("rename"), Some("="), Some(variant_name_lit))
                    if variant_name_lit.starts_with('\"') && variant_name_lit.ends_with('\"') =>
                {
                    *variant_name = variant_name_lit[1..variant_name_lit.len() - 1].to_owned();
                    break;
                }
                _ => {}
            }
        };
    }
}
