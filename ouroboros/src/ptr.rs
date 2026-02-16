use std::fmt::{self, Display, Formatter};

use crate::{Error, Result, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ptr {
    pub t: Box<Type>,
}

impl Ptr {
    pub fn new(t: impl Into<Type>) -> Self {
        Self {
            t: Box::new(t.into()),
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match value {
            Some(value) => {
                if value.is_string() {
                    Ok(())
                } else {
                    Err(Error::InvalidPtr {
                        expected: self.clone(),
                        e: Error::UnexpectedValue { got: value.clone() }.into(),
                    })
                }
            }
            None => Err(Error::InvalidPtr {
                expected: self.clone(),
                e: Error::UnexpectedNull.into(),
            }),
        }
    }
}

impl Display for Ptr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "@".fmt(f)?;
        self.t.fmt(f)
    }
}

#[cfg(test)]
mod test {
    use crate::{Record, TypeInfo};

    use super::*;

    #[test]
    fn test_ptr_display() {
        assert_eq!(Ptr::new(<()>::t()).to_string(), "@()");
        assert_eq!(Ptr::new(i32::t()).to_string(), "@i32");
        assert_eq!(Ptr::new(u32::t()).to_string(), "@u32");
        assert_eq!(Ptr::new(f32::t()).to_string(), "@f32");
        assert_eq!(Ptr::new(<Vec<i32>>::t()).to_string(), "@[i32]");
    }

    #[test]
    fn test_ptr_compat() {
        assert_eq!(
            Ptr::new(<()>::t()).is_compat(Some(&serde_json::Value::String(
                "any-kind-of-ptr-value-goes-here".into()
            ))),
            Ok(())
        );
        assert_eq!(
            Ptr::new(<()>::t()).is_compat(Some(&serde_json::Value::Null)),
            Err(Error::InvalidPtr {
                expected: Ptr::new(<()>::t()),
                e: Error::UnexpectedValue {
                    got: serde_json::Value::Null
                }
                .into(),
            })
        );
        assert_eq!(
            Ptr::new(<()>::t()).is_compat(Some(&serde_json::Value::Bool(false))),
            Err(Error::InvalidPtr {
                expected: Ptr::new(<()>::t()),
                e: Error::UnexpectedValue {
                    got: serde_json::Value::Bool(false)
                }
                .into(),
            })
        );
        assert_eq!(
            Ptr::new(<()>::t()).is_compat(None),
            Err(Error::InvalidPtr {
                expected: Ptr::new(<()>::t()),
                e: Error::UnexpectedNull.into(),
            })
        );
        assert_eq!(
            Ptr::new(Type::Record(Record::new(
                "Foo",
                [("x", i32::t()), ("y", u32::t()), ("z", f32::t())]
            )))
            .is_compat(Some(&serde_json::Value::String(
                "any-kind-of-ptr-value-goes-here".into()
            ))),
            Ok(())
        );
    }
}
