use std::fmt::{self, Display, Formatter};

use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbolic {
    pub doc: Option<String>,
    pub n: String,
}

impl Symbolic {
    pub fn new(n: impl Into<String>) -> Self {
        Self {
            doc: None,
            n: n.into(),
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match value {
            Some(value) => {
                if value.is_string() {
                    Ok(())
                } else {
                    Err(Error::InvalidSymbolic {
                        expected: self.clone(),
                        e: Error::UnexpectedValue { got: value.clone() }.into(),
                    })
                }
            }
            None => Err(Error::InvalidSymbolic {
                expected: self.clone(),
                e: Error::UnexpectedNull.into(),
            }),
        }
    }
}

impl Display for Symbolic {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "$".fmt(f)?;
        self.n.fmt(f)
    }
}
