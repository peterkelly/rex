use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::{Result, Type, TypeInfo, TypeName};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generic {
    pub n: String,
}

impl Generic {
    pub fn new(n: impl Into<String>) -> Self {
        Self { n: n.into() }
    }

    pub fn is_compat(&self, _value: Option<&serde_json::Value>) -> Result<()> {
        // Everything is compatible with a generic. However, consider that once
        // a generic is resolved, it is no longer compatible with anything. This
        // check needs to be done at a higher-level.
        Ok(())
    }
}

impl Display for Generic {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "^".fmt(f)?;
        self.n.fmt(f)
    }
}

macro_rules! generic {
    ($id: ident) => {
        #[derive(Deserialize, Serialize)]
        pub struct $id(serde_json::Value);

        impl $id {
            pub fn new<V>(v: &V) -> Self
            where
                V: Serialize,
            {
                Self(serde_json::to_value(v).expect("invalid json"))
            }
        }

        impl TypeInfo for $id {
            fn tname() -> TypeName {
                TypeName {
                    n: stringify!($id),
                    g: vec![],
                }
            }

            fn t() -> Type {
                Type::Generic(Generic::new(stringify!($id)))
            }
        }
    };
}

generic!(A);
generic!(B);
generic!(C);
generic!(D);
generic!(E);
generic!(F);
generic!(G);
generic!(H);
generic!(I);
generic!(J);
generic!(K);
generic!(L);
generic!(M);
generic!(N);
generic!(O);
generic!(P);
generic!(Q);
generic!(R);
generic!(S);
generic!(T);
generic!(U);
generic!(V);
generic!(W);
generic!(X);
generic!(Y);
generic!(Z);
