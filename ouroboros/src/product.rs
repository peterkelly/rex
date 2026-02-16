use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
};

use crate::{
    field::{Fields, UnnamedField},
    type_info::Type,
    Error, Result, UnnamedFields,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Array {
    pub t: Box<Type>,
}

impl Array {
    pub fn new(t: impl Into<Type>) -> Self {
        Self {
            t: Box::new(t.into()),
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match value {
            Some(value) => value
                .as_array()
                .map(|a| {
                    a.iter()
                        .try_for_each(|v| self.t.is_compat(Some(v)))
                        .map_err(|e| Error::InvalidArray {
                            expected: self.clone(),
                            e: e.into(),
                        })
                })
                .unwrap_or(Err(Error::InvalidArray {
                    expected: self.clone(),
                    e: Error::UnexpectedValue { got: value.clone() }.into(),
                })),
            None => Err(Error::InvalidArray {
                expected: self.clone(),
                e: Error::UnexpectedNull.into(),
            }),
        }
    }
}

impl Display for Array {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "[".fmt(f)?;
        self.t.fmt(f)?;
        "]".fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Func {
    pub a: Box<Type>,
    pub b: Box<Type>,
}

impl Func {
    pub fn new(a: impl Into<Type>, b: impl Into<Type>) -> Self {
        Self {
            a: Box::new(a.into()),
            b: Box::new(b.into()),
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match value {
            Some(value) => value
                .as_object()
                .and_then(|object| object.get("λ"))
                .and_then(|n| n.as_str())
                .map(|_| Ok(()))
                .unwrap_or(Err(Error::InvalidFunc {
                    expected: self.clone(),
                    e: Error::UnexpectedValue { got: value.clone() }.into(),
                })),
            None => Err(Error::InvalidFunc {
                expected: self.clone(),
                e: Error::UnexpectedNull.into(),
            }),
        }
    }
}

impl Display for Func {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "(".fmt(f)?;
        self.a.fmt(f)?;
        " -> ".fmt(f)?;
        self.b.fmt(f)?;
        ")".fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordDocs {
    pub record: Option<String>,
    pub fields: Option<RecordFieldDocs>,
}

impl RecordDocs {
    pub fn named(record: Option<String>, fields: impl Into<HashMap<String, String>>) -> Self {
        Self {
            record,
            fields: Some(RecordFieldDocs::Named(fields.into())),
        }
    }
}

impl From<&str> for RecordDocs {
    fn from(record: &str) -> Self {
        Self {
            record: Some(record.to_string()),
            fields: None,
        }
    }
}

impl<const N: usize> From<(&str, [(&str, &str); N])> for RecordDocs {
    fn from((record, fields): (&str, [(&str, &str); N])) -> Self {
        Self {
            record: Some(record.to_string()),
            fields: Some(RecordFieldDocs::Named(
                fields
                    .iter()
                    .map(|(n, doc)| (n.to_string(), doc.to_string()))
                    .collect(),
            )),
        }
    }
}

impl<const N: usize> From<[(&str, &str); N]> for RecordDocs {
    fn from(fields: [(&str, &str); N]) -> Self {
        Self {
            record: None,
            fields: Some(RecordFieldDocs::Named(
                fields
                    .iter()
                    .map(|(n, doc)| (n.to_string(), doc.to_string()))
                    .collect(),
            )),
        }
    }
}

impl<const N: usize> From<(&str, [&str; N])> for RecordDocs {
    fn from((record, fields): (&str, [&str; N])) -> Self {
        Self {
            record: Some(record.to_string()),
            fields: Some(RecordFieldDocs::Unnamed(
                fields.iter().map(|s| Some(s.to_string())).collect(),
            )),
        }
    }
}

impl<const N: usize> From<[&str; N]> for RecordDocs {
    fn from(fields: [&str; N]) -> Self {
        Self {
            record: None,
            fields: Some(RecordFieldDocs::Unnamed(
                fields.iter().map(|s| Some(s.to_string())).collect(),
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordFieldDocs {
    Named(HashMap<String, String>),
    Unnamed(Vec<Option<String>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub doc: Option<RecordDocs>,
    pub n: String,
    pub fields: Fields,
}

impl Record {
    pub fn new_unit(n: impl Into<String>) -> Self {
        Self {
            doc: None,
            n: n.into(),
            fields: Vec::<UnnamedField>::new().into(),
        }
    }

    pub fn new(n: impl Into<String>, fields: impl Into<Fields>) -> Self {
        Self {
            doc: None,
            n: n.into(),
            fields: fields.into(),
        }
    }

    pub fn with_doc(
        doc: impl Into<RecordDocs>,
        n: impl Into<String>,
        fields: impl Into<Fields>,
    ) -> Self {
        Self {
            doc: Some(doc.into()),
            n: n.into(),
            fields: fields.into(),
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        self.fields
            .is_compat(value)
            .map_err(|e| Error::InvalidRecord {
                expected: self.clone(),
                e: e.into(),
            })
    }
}

impl Display for Record {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.n.fmt(f)?;
        self.fields.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tuple {
    pub fields: UnnamedFields,
}

impl Tuple {
    pub fn new(fields: impl Into<UnnamedFields>) -> Self {
        Self {
            fields: fields.into(),
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match value {
            Some(value) => value
                .as_array()
                .and_then(|array| {
                    (array.len() == self.fields.len()).then(|| {
                        self.fields
                            .iter()
                            .zip(array.iter())
                            .enumerate()
                            .try_for_each(|(i, (f, v))| {
                                f.t.is_compat(Some(v))
                                    .map_err(|e| Error::InvalidUnnamedField {
                                        index: i,
                                        e: e.into(),
                                    })
                            })
                    })
                })
                .unwrap_or(Err(Error::InvalidFields {
                    expected: Fields::Unnamed(self.fields.clone()),
                    e: Error::UnexpectedValue { got: value.clone() }.into(),
                }))
                .map_err(|e| Error::InvalidTuple {
                    expected: self.clone(),
                    e: e.into(),
                }),
            None => Err(Error::InvalidTuple {
                expected: self.clone(),
                e: Error::UnexpectedNull.into(),
            }),
        }
    }
}

impl Display for Tuple {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "(".fmt(f)?;
        for (i, field) in self.fields.iter().enumerate() {
            if i > 0 {
                ", ".fmt(f)?;
            }
            field.t.fmt(f)?;
        }
        ")".fmt(f)
    }
}
