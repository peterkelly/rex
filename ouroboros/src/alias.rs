use std::fmt::{self, Display, Formatter};

use crate::{Error, Result, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    pub n: String,
    pub t: Box<Type>,
}

impl Alias {
    pub fn new(n: impl Into<String>, t: impl Into<Type>) -> Self {
        Self {
            n: n.into(),
            t: Box::new(t.into()),
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        self.t.is_compat(value).map_err(|e| Error::InvalidAlias {
            expected: self.clone(),
            e: e.into(),
        })
    }
}

impl Display for Alias {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.n.fmt(f)?;
        "(".fmt(f)?;
        self.t.fmt(f)?;
        ")".fmt(f)
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use super::*;
    use crate::{Array, TypeInfo};

    #[test]
    fn test_alias_display() {
        let alias = Alias::new("Foo", <()>::t());
        assert_eq!(alias.to_string(), "Foo(())");

        let alias = Alias::new("Foo", i32::t());
        assert_eq!(alias.to_string(), "Foo(i32)");

        let alias = Alias::new("Foo", u32::t());
        assert_eq!(alias.to_string(), "Foo(u32)");

        let alias = Alias::new("Foo", f32::t());
        assert_eq!(alias.to_string(), "Foo(f32)");

        let alias = Alias::new("Foo", <Vec<i32>>::t());
        assert_eq!(alias.to_string(), "Foo([i32])");
    }

    #[test]
    fn test_alias_compat() {
        let alias = Alias::new("Foo", <()>::t());
        assert_eq!(alias.is_compat(Some(&serde_json::Value::Null)), Ok(()));

        let alias = Alias::new("Foo", i32::t());
        assert_eq!(
            alias.is_compat(Some(&serde_json::Value::Number(69i32.into()))),
            Ok(())
        );
        assert_eq!(
            alias.is_compat(Some(&serde_json::Value::String("69".into()))),
            Err(Error::InvalidAlias {
                expected: alias.clone(),
                e: Box::new(Error::InvalidI32 {
                    got: serde_json::Value::String("69".into())
                }),
            })
        );

        let alias = Alias::new("Foo", <Vec<i32>>::t());
        assert_eq!(alias.is_compat(Some(&json!([1, 2, 3]))), Ok(()));
        assert_eq!(
            alias.is_compat(Some(&json!(["1", "2", "3"]))),
            Err(Error::InvalidAlias {
                expected: alias.clone(),
                e: Box::new(Error::InvalidArray {
                    expected: Array::new(i32::t()),
                    e: Box::new(Error::InvalidI32 {
                        got: serde_json::Value::String("1".into())
                    }),
                }),
            })
        );
    }
}
