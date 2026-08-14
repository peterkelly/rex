use blake3::Hash;
use rex::{
    ast::Symbol,
    engine::{EngineError, FromRex, IntoRex, Module, Value, virtual_export_name},
    typesystem::{AdtArgument, AdtDecl, AdtField, RexAdt, RexType, Type, TypeVarSupply},
};

use crate::state::State;

const MODULE_NAME: &str = "artifacts";

macro_rules! artifact_type {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name {
            /// Hash of the content-addressed blob containing this artifact.
            pub content: Hash,
        }

        impl RexType for $name {
            fn rex_type() -> Type {
                Type::user_con(virtual_export_name(MODULE_NAME, stringify!($name)), 0)
            }

            // The declaration is owned and eagerly installed by `artifacts::module`. Leaving the
            // default family collection empty prevents a tool signature from recreating this ADT
            // as a tool-local type.
        }

        impl RexAdt for $name {
            fn rex_adt_decl() -> Result<AdtDecl, rex::typesystem::TypeError> {
                let mut supply = TypeVarSupply::new();
                let mut adt = AdtDecl::new(&Symbol::intern(stringify!($name)), &[], &mut supply);
                adt.docs = Some($docs.to_owned());
                adt.add_variant(
                    Symbol::intern(stringify!($name)),
                    vec![AdtArgument::Record {
                        fields: vec![AdtField {
                            name: Symbol::intern("content"),
                            typ: <Hash as RexType>::rex_type(),
                            docs: Some(
                                "Hash of the content-addressed blob containing this artifact."
                                    .to_owned(),
                            ),
                        }],
                        docs: None,
                    }],
                    Some($docs.to_owned()),
                );
                Ok(adt)
            }

            fn rex_adt_family() -> Result<Vec<AdtDecl>, rex::typesystem::TypeError> {
                Ok(vec![Self::rex_adt_decl()?])
            }
        }

        impl IntoRex for $name {
            fn into_rex(self) -> Result<Value, EngineError> {
                Ok(Value::Adt(
                    Symbol::intern(stringify!($name)),
                    vec![Value::Dict(std::collections::BTreeMap::from([(
                        "content".to_owned(),
                        self.content.into_rex()?,
                    )]))],
                ))
            }
        }

        impl FromRex for $name {
            fn from_rex(value: Value) -> Result<Self, EngineError> {
                let got = value.value_type_name().to_owned();
                match value {
                    Value::Adt(tag, mut args)
                        if tag.as_ref() == stringify!($name) && args.len() == 1 =>
                    {
                        let value = args.pop().ok_or_else(|| {
                            EngineError::Internal(format!(
                                "validated {} value had no record argument",
                                stringify!($name)
                            ))
                        })?;
                        let Value::Dict(mut fields) = value else {
                            return Err(EngineError::NativeType {
                                expected: "dict".into(),
                                got: value.value_type_name().into(),
                            });
                        };
                        let content =
                            fields
                                .remove("content")
                                .ok_or_else(|| EngineError::NativeType {
                                    expected: "missing field `content`".into(),
                                    got: "dict".into(),
                                })?;
                        Ok(Self {
                            content: Hash::from_rex(content)?,
                        })
                    }
                    _ => Err(EngineError::NativeType {
                        expected: stringify!($name).into(),
                        got,
                    }),
                }
            }
        }
    };
}

artifact_type!(Pdf, "One PDF document stored as a content-addressed blob.");
artifact_type!(
    Image,
    "One encoded image stored as a content-addressed blob; it may contain multiple frames."
);
artifact_type!(
    Media,
    "One encoded audio, video, subtitle, or mixed-media file stored as a content-addressed blob."
);
artifact_type!(
    JsonFile,
    "One UTF-8 JSON document stored as a content-addressed blob."
);

/// Build the shared semantic artifact module.
///
/// The module owns artifact types used across workflow tool APIs. Constructing a wrapper classifies
/// a content hash but does not inspect its bytes; the consuming tool validates that the stored blob
/// is a supported input. Inject this module before installing any workflow tool module.
pub fn module() -> Result<Module<State>, EngineError> {
    let mut module = Module::new(
        MODULE_NAME,
        Some(
            "Shared semantic types for content-addressed workflow artifacts.\n\nArtifact wrappers carry CAS hashes rather than host paths. Constructing a wrapper classifies a blob but does not inspect it; consuming tools validate the stored content."
                .to_owned(),
        ),
    );
    module.add_adt_decl(Pdf::rex_adt_decl()?)?;
    module.add_adt_decl(Image::rex_adt_decl()?)?;
    module.add_adt_decl(Media::rex_adt_decl()?)?;
    module.add_adt_decl(JsonFile::rex_adt_decl()?)?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn artifact_types_have_canonical_ownership() {
        let module = module().unwrap();
        assert_eq!(module.name(), MODULE_NAME);
        assert_eq!(module.adts().len(), 4);
        assert_eq!(
            Pdf::rex_type(),
            Type::user_con(virtual_export_name(MODULE_NAME, "Pdf"), 0)
        );
        assert_eq!(Pdf::rex_adt_family().unwrap().len(), 1);
        assert!(module.adts().iter().all(|staged| {
            staged
                .adt
                .docs
                .as_deref()
                .is_some_and(|docs| !docs.trim().is_empty())
        }));
    }

    #[test]
    fn artifact_values_roundtrip_through_host_conversion() {
        let expected = Pdf {
            content: Hash::from_str(
                "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f",
            )
            .unwrap(),
        };
        let value = expected.clone().into_rex().unwrap();
        assert_eq!(Pdf::from_rex(value).unwrap(), expected);
    }
}
