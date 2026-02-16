#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(unused_mut)]
#![allow(unused_assignments)]
#![allow(unused_imports)]
#![allow(unused_macros)]
#![allow(non_upper_case_globals)]
//
// NOTE ON LOSSINESS:
// - `ouroboros -> rex` is mostly straightforward.
// - `rex -> ouroboros` is structurally lossy for nominal ADTs in Rex v4 (`Type::con(...)`):
//   records/enums/unions/symbolics with the same constructor shape cannot always be
//   distinguished without out-of-band metadata.
// - Enum discriminant values and docs are not recoverable from Rex types.
// - Named vs unnamed record identity is only preserved when Rex uses `Type::Record`; for
//   nominal `Type::con("Foo", 0)` values, reverse conversion defaults to `Symbolic("Foo")`.

use ouroboros::alias::Alias;
use ouroboros::field::{Fields, NamedField, NamedFields, UnnamedField, UnnamedFields};
use ouroboros::generic::Generic;
use ouroboros::product::{Array, Func, Record, RecordDocs, RecordFieldDocs, Tuple};
use ouroboros::ptr::Ptr;
use ouroboros::sum::{Enum, EnumVariant, Fallible, Optional, Union, UnionVariant};
use ouroboros::symbolic::Symbolic;
use ouroboros::type_info::Type as OuroborosType;
use rex::{RexType, TypeSystem, sym};
use rex_ts::TypeKind;
use std::collections::{BTreeMap, HashMap};

#[derive(Default)]
pub struct JsonOptions;

// Note: We cannot use From/Into here as these only work if the types
// they are implemented on are defined in the same crate

pub fn ouroboros_to_rex(
    ts: &mut TypeSystem,
    our_type: &OuroborosType,
    opts: &JsonOptions,
) -> Result<rex::Type, String> {
    match our_type {
        OuroborosType::Unit => Ok(<()>::rex_type()),
        OuroborosType::Bool => Ok(bool::rex_type()),
        OuroborosType::I8 => Ok(i8::rex_type()),
        OuroborosType::I16 => Ok(i16::rex_type()),
        OuroborosType::I32 => Ok(i32::rex_type()),
        OuroborosType::I64 => Ok(i64::rex_type()),
        OuroborosType::I128 => Err("Unsupported ouroboros type: I128".to_string()),
        OuroborosType::U8 => Ok(u8::rex_type()),
        OuroborosType::U16 => Ok(u16::rex_type()),
        OuroborosType::U32 => Ok(u32::rex_type()),
        OuroborosType::U64 => Ok(u64::rex_type()),
        OuroborosType::U128 => Err("Unsupported ouroboros type: U128".to_string()),
        OuroborosType::F32 => Ok(f32::rex_type()),
        OuroborosType::F64 => Ok(f64::rex_type()),
        OuroborosType::String => Ok(String::rex_type()),
        OuroborosType::Array(a) => Ok(rex::Type::array(ouroboros_to_rex(ts, &a.t, opts)?)),
        OuroborosType::Record(r) => Ok(rex::Type::con(&r.n, 0)),
        OuroborosType::Tuple(tuple) => unnamed_fields_to_rex(ts, &tuple.fields, opts),
        OuroborosType::Func(func) => Ok(rex::Type::fun(
            ouroboros_to_rex(ts, &func.a, opts)?,
            ouroboros_to_rex(ts, &func.b, opts)?,
        )),
        OuroborosType::Enum(e) => Ok(rex::Type::con(&e.n, 0)),
        OuroborosType::Fallible(f) => Ok(rex::Type::result(
            ouroboros_to_rex(ts, &f.err, opts)?,
            ouroboros_to_rex(ts, &f.ok, opts)?,
        )),
        OuroborosType::Optional(o) => Ok(rex::Type::option(ouroboros_to_rex(ts, &o.t, opts)?)),
        OuroborosType::Union(u) => Ok(rex::Type::con(&u.n, 0)),
        OuroborosType::Ptr(_) => Ok(rex::Type::con("Ptr", 0)),
        OuroborosType::Symbolic(symbolic) => Ok(rex::Type::con(&symbolic.n, 0)),
        OuroborosType::Generic(g) => Ok(rex::Type::var(ts.supply.fresh(Some(sym(&g.n))))),
        OuroborosType::Alias(alias) => ouroboros_to_rex(ts, &alias.t, opts),
    }
}

fn fields_to_rex(
    ts: &mut TypeSystem,
    fields: &Fields,
    opts: &JsonOptions,
) -> Result<rex::Type, String> {
    match fields {
        Fields::Named(named) => named_fields_to_rex(ts, named, opts),
        Fields::Unnamed(unnamed) => unnamed_fields_to_rex(ts, unnamed, opts),
    }
}

fn named_fields_to_rex(
    ts: &mut TypeSystem,
    fields: &NamedFields,
    opts: &JsonOptions,
) -> Result<rex::Type, String> {
    let mut entries = Vec::new();
    for field in fields.fields.iter() {
        entries.push((sym(&field.n), ouroboros_to_rex(ts, &field.t, opts)?));
    }
    Ok(rex::Type::record(entries))
}

fn unnamed_fields_to_rex(
    ts: &mut TypeSystem,
    fields: &UnnamedFields,
    opts: &JsonOptions,
) -> Result<rex::Type, String> {
    let mut rex_types: Vec<rex::Type> = Vec::new();
    for field in fields.iter() {
        rex_types.push(ouroboros_to_rex(ts, &field.t, opts)?);
    }
    Ok(rex::Type::tuple(rex_types))
}

// fn get_discriminant(v: &EnumVariant, patches: Option<&Vec<EnumPatch>>) -> Option<i64> {
//     let mut discriminant: Option<i64> = None;

//     if let Some(patches) = patches {
//         for p in patches.iter() {
//             if p.enum_name == v.n {
//                 discriminant = Some(p.discriminant);
//             }
//         }
//     }

//     if discriminant.is_none()
//         && let Some(d) = v.v {
//             discriminant = Some(d.into());
//         }
//     discriminant
// }

// fn rex_to_fields(rex_type: &rex::Type) -> Result<Fields, String> {
//     match rex_type {
//         rex::Type::Tuple(rex_types) => {
//             let mut our_fields: Vec<UnnamedField> = Vec::new();
//             for t in rex_types.iter() {
//                 our_fields.push(UnnamedField {
//                     t: rex_to_ouroboros(t)?,
//                 });
//             }
//             Ok(Fields::Unnamed(UnnamedFields { fields: our_fields }))
//         }
//         rex::Type::Dict(entries) => {
//             let mut our_fields: Vec<NamedField> = Vec::new();
//             for (k, t) in entries.iter() {
//                 our_fields.push(NamedField {
//                     n: k.clone(),
//                     t: rex_to_ouroboros(t)?,
//                 });
//             }
//             Ok(Fields::Named(NamedFields { fields: our_fields }))
//         }
//         _ => Ok(Fields::Unnamed(UnnamedFields {
//             fields: vec![UnnamedField {
//                 t: rex_to_ouroboros(rex_type)?,
//             }],
//         })),
//     }
// }

pub fn rex_to_ouroboros(rex_type: &rex::Type) -> Result<OuroborosType, String> {
    match rex_type.as_ref() {
        TypeKind::Var(tv) => Ok(OuroborosType::Generic(Generic::new(
            tv.name
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("t{}", tv.id)),
        ))),
        TypeKind::Con(c) => rex_con_to_ouroboros(c.name.as_ref()),
        TypeKind::App(_, _) => rex_app_to_ouroboros(rex_type),
        TypeKind::Fun(a, b) => Ok(OuroborosType::Func(Func {
            a: Box::new(rex_to_ouroboros(a)?),
            b: Box::new(rex_to_ouroboros(b)?),
        })),
        TypeKind::Tuple(ts) => {
            if ts.is_empty() {
                Ok(OuroborosType::Unit)
            } else {
                let mut fields: Vec<UnnamedField> = Vec::new();
                for t in ts.iter() {
                    fields.push(UnnamedField::new(rex_to_ouroboros(t)?));
                }
                Ok(OuroborosType::Tuple(Tuple {
                    fields: UnnamedFields { fields },
                }))
            }
        }
        TypeKind::Record(entries) => {
            let mut fields: Vec<NamedField> = Vec::new();
            for (name, t) in entries {
                fields.push(NamedField {
                    n: name.to_string(),
                    t: rex_to_ouroboros(t)?,
                });
            }
            Ok(OuroborosType::Record(Record {
                doc: None,
                n: "Record".to_string(),
                fields: Fields::Named(NamedFields { fields }),
            }))
        }
    }
}

fn rex_con_to_ouroboros(name: &str) -> Result<OuroborosType, String> {
    match name {
        "bool" => Ok(OuroborosType::Bool),
        "i8" => Ok(OuroborosType::I8),
        "i16" => Ok(OuroborosType::I16),
        "i32" => Ok(OuroborosType::I32),
        "i64" => Ok(OuroborosType::I64),
        "u8" => Ok(OuroborosType::U8),
        "u16" => Ok(OuroborosType::U16),
        "u32" => Ok(OuroborosType::U32),
        "u64" => Ok(OuroborosType::U64),
        "f32" => Ok(OuroborosType::F32),
        "f64" => Ok(OuroborosType::F64),
        "string" => Ok(OuroborosType::String),
        "uuid" => Err("Unsupported Rex type: uuid".to_string()),
        "datetime" => Err("Unsupported Rex type: datetime".to_string()),
        _ => Ok(OuroborosType::Symbolic(Symbolic {
            n: name.to_string(),
            doc: None,
        })),
    }
}

fn rex_app_to_ouroboros(rex_type: &rex::Type) -> Result<OuroborosType, String> {
    let mut args: Vec<&rex::Type> = Vec::new();
    let mut head = rex_type;
    while let TypeKind::App(f, arg) = head.as_ref() {
        args.push(arg);
        head = f;
    }
    args.reverse();

    let TypeKind::Con(c) = head.as_ref() else {
        return Err("Unsupported Rex type: non-constructor application".to_string());
    };
    match (c.name.as_ref(), args.as_slice()) {
        ("Array", [elem]) => Ok(OuroborosType::Array(Array::new(rex_to_ouroboros(elem)?))),
        ("Option", [elem]) => Ok(OuroborosType::Optional(Optional {
            t: Box::new(rex_to_ouroboros(elem)?),
        })),
        ("Result", [err, ok]) => Ok(OuroborosType::Fallible(Fallible {
            ok: Box::new(rex_to_ouroboros(ok)?),
            err: Box::new(rex_to_ouroboros(err)?),
        })),
        _ => Err(format!(
            "Unsupported Rex type application: {} with {} args",
            c.name,
            args.len()
        )),
    }
}

// fn rex_adt_to_ouroboros(adt: &ADT) -> Result<OuroborosType, String> {
//     if adt.variants.len() == 1 {
//         match adt.variants[0].t.as_deref() {
//             // ADT with a single variant whose type is a dict -> Record w/named fields
//             Some(rex::Type::Dict(entries)) => {
//                 return rex_record_to_ouroboros(&adt.name, &adt.variants[0].t_docs, entries);
//             }
//             // ADT with a single variant whose type is a tuple -> Record w/unnamed fields
//             Some(rex::Type::Tuple(entries)) => {
//                 let mut fields: Vec<UnnamedField> = Vec::new();
//                 for entry in entries.iter() {
//                     fields.push(UnnamedField {
//                         t: rex_to_ouroboros(entry)?,
//                     });
//                 }
//                 return Ok(OuroborosType::Record(Record {
//                     doc: None,
//                     n: adt.name.clone(),
//                     fields: Fields::Unnamed(UnnamedFields { fields }),
//                 }));
//             }
//             // ADT with a single variant of another type -> Alias
//             Some(t) => {
//                 if adt.name == "Ptr" && adt.variants[0].name == "Ptr" {
//                     return Ok(OuroborosType::Ptr(Ptr {
//                         t: Box::new(rex_to_ouroboros(t)?),
//                     }));
//                 }

//                 return Ok(OuroborosType::Alias(Alias {
//                     n: adt.name.clone(),
//                     t: Box::new(rex_to_ouroboros(t)?),
//                 }));
//             }
//             None => {
//                 if adt.variants[0].name == adt.name {
//                     return Ok(OuroborosType::Symbolic(Symbolic {
//                         n: adt.name.clone(),
//                         doc: adt.docs.clone(),
//                     }));
//                 }
//             }
//         }
//     }

//     // ADT where no member carries any data value -> Enum
//     if adt.variants.iter().all(|v| v.t.is_none()) {
//         return rex_enum_to_ouroboros(&adt.name, &adt.variants);
//     }

//     // General ADT
//     let mut our_variants: Vec<UnionVariant> = Vec::new();
//     for rex_variant in adt.variants.iter() {
//         let fields: Option<Fields> = match &rex_variant.t {
//             Some(rex_fields) => Some(rex_to_fields(rex_fields)?),
//             None => None,
//         };
//         our_variants.push(UnionVariant {
//             n: rex_variant.name.clone(),
//             fields,
//         })
//     }
//     let union_ = Union {
//         doc: None,
//         n: adt.name.clone(),
//         variants: our_variants,
//     };
//     Ok(OuroborosType::Union(union_))
// }

// fn rex_record_to_ouroboros(
//     name: &str,
//     t_docs: &Option<BTreeMap<String, String>>,
//     entries: &BTreeMap<String, Arc<rex::Type>>,
// ) -> Result<OuroborosType, String> {
//     // Match behaviour of #[derive(TypeInfo)] - use empty hashmap for field docs if
//     // there are no actual docs present
//     let hm: HashMap<String, String> = match t_docs {
//         Some(entries) => HashMap::from_iter(entries.clone()),
//         None => HashMap::new(),
//     };
//     let doc: Option<RecordDocs> = Some(RecordDocs {
//         record: None,
//         fields: Some(RecordFieldDocs::Named(hm)),
//     });

//     let mut fields: Vec<NamedField> = Vec::new();
//     for (k, v) in entries.iter() {
//         fields.push(NamedField {
//             n: k.clone(),
//             t: rex_to_ouroboros(v)?,
//         });
//     }

//     Ok(OuroborosType::Record(Record {
//         doc,
//         n: name.to_string(),
//         fields: Fields::Named(NamedFields { fields }),
//     }))
// }

// fn rex_enum_to_ouroboros(name: &str, variants: &[ADTVariant]) -> Result<OuroborosType, String> {
//     Ok(OuroborosType::Enum(Enum {
//         doc: None,
//         n: name.to_string(),
//         variants: variants
//             .iter()
//             .map(|v| EnumVariant {
//                 n: v.name.clone(),
//                 v: None,
//             })
//             .collect(),
//     }))
// }

#[cfg(test)]
pub mod test {
    #![allow(dead_code)]
    use super::*;
    use ouroboros::{ptr::Ptr as OuroborosPtr, type_info::TypeInfo};
    use ouroboros_proc_macro::*;
    use rex::{Rex, RexType};
    // use rex_type_system::types as rex;

    fn symbolic(name: &str) -> OuroborosType {
        OuroborosType::Symbolic(Symbolic {
            n: name.to_string(),
            doc: None,
        })
    }

    #[test]
    fn test_unit() {
        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &OuroborosType::Unit, &Default::default()).unwrap(),
            rex::Type::tuple(vec![])
        );
        assert_eq!(
            rex_to_ouroboros(&rex::Type::tuple(vec![])).unwrap(),
            OuroborosType::Unit
        )
    }

    #[test]
    fn test_tuple1() {
        let our_type = OuroborosType::Tuple(Tuple {
            fields: UnnamedFields {
                fields: vec![UnnamedField {
                    t: OuroborosType::U64,
                }],
            },
        });
        let mut ts = TypeSystem::new();
        let rex_type = ouroboros_to_rex(&mut ts, &our_type, &Default::default()).unwrap();
        assert_eq!(rex_type, rex::Type::tuple(vec![rex::Type::con("u64", 0)]));
        assert_eq!(rex_to_ouroboros(&rex_type).unwrap(), our_type);
    }

    #[test]
    fn test_tuple2() {
        let our_type = OuroborosType::Tuple(Tuple {
            fields: UnnamedFields {
                fields: vec![
                    UnnamedField {
                        t: OuroborosType::U64,
                    },
                    UnnamedField {
                        t: OuroborosType::String,
                    },
                ],
            },
        });
        let mut ts = TypeSystem::new();
        let rex_type = ouroboros_to_rex(&mut ts, &our_type, &Default::default()).unwrap();
        assert_eq!(
            rex_type,
            rex::Type::tuple(vec![rex::Type::con("u64", 0), rex::Type::con("string", 0)])
        );
        assert_eq!(rex_to_ouroboros(&rex_type).unwrap(), our_type);
    }

    #[test]
    fn test_record_named() {
        #[derive(Rex, TypeInfo)]
        struct Foo {
            /// First field
            a: u64,
            /// Second field
            b: String,
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"))
    }

    #[test]
    fn test_record_unnamed() {
        #[derive(Rex, TypeInfo)]
        pub struct Foo(pub u64, pub String);

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"))
    }

    #[test]
    fn test_enum() {
        #[derive(Rex, TypeInfo)]
        enum Foo {
            One,
            Two,
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"));
    }

    #[test]
    fn test_enum_int() {
        mod int {
            use ouroboros_proc_macro::*;
            use rex::Rex;

            #[derive(Rex, TypeInfo)]
            pub enum Foo {
                One = 1,
                Two = 2,
                Three = 3,
            }
        }

        mod plain {
            use ouroboros_proc_macro::*;
            use rex::Rex;

            #[derive(Rex, TypeInfo)]
            pub enum Foo {
                One,
                Two,
                Three,
            }
        }

        // Rex's ADTVariant doesn't keep track of the integer values associated with enum
        // variants. So when we convert back to the ouroboros type, we'll get the same result
        // as derive(TypeInfo) would produce if there were no integer values in the enum.
        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &int::Foo::t(), &Default::default()).unwrap(),
            int::Foo::rex_type()
        );
        assert_eq!(
            rex_to_ouroboros(&int::Foo::rex_type()).unwrap(),
            symbolic("Foo")
        );
    }

    #[test]
    fn test_union_single_field() {
        #[derive(Rex, TypeInfo)]
        enum Foo {
            One(u64),
            Two(String),
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"));
    }

    #[test]
    fn test_union_multiple_fields() {
        #[derive(Rex, TypeInfo)]
        enum Foo {
            One(u64, bool),
            Two(String, f64),
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"));
    }

    #[test]
    fn test_union_mixed() {
        #[derive(Rex, TypeInfo)]
        enum Foo {
            One,
            Two(u64),
            Three(String, f64),
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"));
    }

    #[test]
    fn test_union_named() {
        #[derive(Rex, TypeInfo)]
        enum Foo {
            One { a: u64, b: String },
            Two { a: f64, b: bool },
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"));
    }

    #[test]
    fn test_numeric_roundtrip() {
        // Rex v4 preserves numeric widths across conversion boundaries.
        mod different_sizes {
            use ouroboros_proc_macro::*;
            use rex::Rex;

            #[derive(Rex, TypeInfo)]
            pub struct Foo {
                pub u1: u8,
                pub u2: u16,
                pub u3: u32,
                pub u4: u64,
                pub i1: i8,
                pub i2: i16,
                pub i3: i32,
                pub i4: i64,
                pub f1: f32,
                pub f2: f64,
            }
        }

        mod all_64 {
            use ouroboros_proc_macro::*;
            use rex::Rex;

            #[derive(Rex, TypeInfo)]
            pub struct Foo {
                pub u1: u64,
                pub u2: u64,
                pub u3: u64,
                pub u4: u64,
                pub i1: i64,
                pub i2: i64,
                pub i3: i64,
                pub i4: i64,
                pub f1: f64,
                pub f2: f64,
            }
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            rex_to_ouroboros(
                &ouroboros_to_rex(&mut ts, &different_sizes::Foo::t(), &Default::default())
                    .unwrap()
            )
            .unwrap(),
            symbolic("Foo")
        );

        assert_eq!(
            ouroboros_to_rex(&mut ts, &different_sizes::Foo::t(), &Default::default()).unwrap(),
            different_sizes::Foo::rex_type()
        );
    }

    #[test]
    fn test_alias() {
        let our_type = OuroborosType::Alias(Alias {
            n: "Foo".to_string(),
            t: Box::new(OuroborosType::U64),
        });

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &our_type, &Default::default()).unwrap(),
            rex::Type::con("u64", 0)
        );
    }

    #[test]
    fn test_symbolic_doc() {
        #[derive(Rex, TypeInfo)]
        /// Test
        pub enum Foo {
            Foo,
        }

        let our_type = OuroborosType::Symbolic(Symbolic {
            n: "Foo".to_string(),
            doc: Some("Test".to_string()),
        });
        let rex_type = Foo::rex_type();

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &our_type, &Default::default()).unwrap(),
            rex_type
        );
        assert_eq!(rex_to_ouroboros(&rex_type).unwrap(), symbolic("Foo"));
    }

    #[test]
    fn test_symbolic_nodoc() {
        #[derive(Rex, TypeInfo)]
        pub enum Foo {
            Foo,
        }

        let our_type = OuroborosType::Symbolic(Symbolic {
            n: "Foo".to_string(),
            doc: None,
        });
        let rex_type = Foo::rex_type();

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &our_type, &Default::default()).unwrap(),
            rex_type
        );
        assert_eq!(rex_to_ouroboros(&rex_type).unwrap(), our_type);
    }

    #[test]
    fn test_ptr() {
        #[derive(Rex)]
        pub enum Ptr {
            Ptr(String),
        }

        let our_type = OuroborosType::Ptr(OuroborosPtr {
            t: Box::new(OuroborosType::String),
        });
        let rex_type = Ptr::rex_type();
        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &our_type, &Default::default()).unwrap(),
            rex_type
        );
        assert_eq!(rex_to_ouroboros(&rex_type).unwrap(), symbolic("Ptr"));
    }

    #[test]
    fn test_func() {
        let our_type = OuroborosType::Func(Func {
            a: Box::new(OuroborosType::U64),
            b: Box::new(OuroborosType::String),
        });
        let rex_type = rex::Type::fun(rex::Type::con("u64", 0), rex::Type::con("string", 0));

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &our_type, &Default::default()).unwrap(),
            rex_type
        );
        assert_eq!(rex_to_ouroboros(&rex_type).unwrap(), our_type);
    }

    #[test]
    fn test_fallible() {
        #[derive(Rex, TypeInfo)]
        pub struct Foo {
            pub a: Result<u64, String>,
        }

        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &Foo::t(), &Default::default()).unwrap(),
            Foo::rex_type()
        );
        assert_eq!(rex_to_ouroboros(&Foo::rex_type()).unwrap(), symbolic("Foo"));
    }

    #[test]
    fn test_generic() {
        let mut ts = TypeSystem::new();
        let tv = ts.supply.fresh(Some(sym("T")));
        let our_generic = OuroborosType::Generic(Generic::new("T"));
        let rex_generic = rex::Type::var(tv.clone());
        let mut ts = TypeSystem::new();
        assert_eq!(
            ouroboros_to_rex(&mut ts, &our_generic, &Default::default()).unwrap(),
            rex_generic
        );
        assert_eq!(rex_to_ouroboros(&rex_generic).unwrap(), our_generic);
    }
}
