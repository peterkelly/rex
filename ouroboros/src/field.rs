use std::vec::IntoIter;
use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
    slice::{Iter, IterMut},
};

use crate::{Error, Result, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedFields {
    pub fields: Vec<NamedField>,
}

impl NamedFields {
    pub fn empty() -> Self {
        Self { fields: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn iter(&self) -> Iter<'_, NamedField> {
        self.fields.iter()
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, NamedField> {
        self.fields.iter_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&NamedField> {
        self.fields.iter().find(|f| f.n == name)
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match value {
            Some(value) => value
                .as_object()
                .map(|object| {
                    self.iter().try_for_each(|f| {
                        f.t.is_compat(object.get(&f.n))
                            .map_err(|e| Error::InvalidNamedField {
                                index: f.n.to_string(),
                                e: e.into(),
                            })
                    })
                })
                .unwrap_or(Err(Error::InvalidFields {
                    expected: Fields::Named(self.clone()),
                    e: Error::UnexpectedValue { got: value.clone() }.into(),
                })),
            _ => Err(Error::InvalidFields {
                expected: Fields::Named(self.clone()),
                e: Error::UnexpectedNull.into(),
            }),
        }
    }
}

impl From<Vec<NamedField>> for NamedFields {
    fn from(fields: Vec<NamedField>) -> Self {
        Self { fields }
    }
}

impl<'a> From<Vec<(&'a str, Type)>> for NamedFields {
    fn from(fields: Vec<(&'a str, Type)>) -> Self {
        Self {
            fields: fields
                .into_iter()
                .map(|(n, t)| NamedField::new(n, t))
                .collect(),
        }
    }
}

impl<const N: usize> From<[NamedField; N]> for NamedFields {
    fn from(fields: [NamedField; N]) -> Self {
        Self {
            fields: fields.into(),
        }
    }
}

impl<'a, const N: usize> From<[(&'a str, Type); N]> for NamedFields {
    fn from(fields: [(&'a str, Type); N]) -> Self {
        Self {
            fields: fields.map(|(n, t)| NamedField::new(n, t)).into(),
        }
    }
}

impl Display for NamedFields {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "{".fmt(f)?;
        for (i, field) in self.fields.iter().enumerate() {
            if i > 0 {
                ", ".fmt(f)?;
            }
            field.fmt(f)?;
        }
        "}".fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedField {
    pub n: String,
    pub t: Type,
}

impl NamedField {
    pub fn new(n: impl Into<String>, t: Type) -> Self {
        Self { n: n.into(), t }
    }
}

impl Display for NamedField {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.n.fmt(f)?;
        ": ".fmt(f)?;
        self.t.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnnamedFields {
    pub fields: Vec<UnnamedField>,
}

impl UnnamedFields {
    pub fn empty() -> Self {
        Self { fields: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn iter(&self) -> Iter<'_, UnnamedField> {
        self.fields.iter()
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, UnnamedField> {
        self.fields.iter_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match value {
            Some(value) => value
                .as_array()
                .and_then(|array| {
                    (array.len() == self.len()).then(|| {
                        self.iter()
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
                    expected: Fields::Unnamed(self.clone()),
                    e: Error::UnexpectedValue { got: value.clone() }.into(),
                })),
            _ => Err(Error::InvalidFields {
                expected: Fields::Unnamed(self.clone()),
                e: Error::UnexpectedNull.into(),
            }),
        }
    }
}

impl IntoIterator for UnnamedFields {
    type Item = UnnamedField;
    type IntoIter = IntoIter<UnnamedField>;

    fn into_iter(self) -> Self::IntoIter {
        self.fields.into_iter()
    }
}

impl From<Vec<UnnamedField>> for UnnamedFields {
    fn from(fields: Vec<UnnamedField>) -> Self {
        Self { fields }
    }
}

impl From<Vec<Type>> for UnnamedFields {
    fn from(fields: Vec<Type>) -> Self {
        Self {
            fields: fields.into_iter().map(UnnamedField::new).collect(),
        }
    }
}

impl<const N: usize> From<[UnnamedField; N]> for UnnamedFields {
    fn from(fields: [UnnamedField; N]) -> Self {
        Self {
            fields: fields.into(),
        }
    }
}

impl<const N: usize> From<[Type; N]> for UnnamedFields {
    fn from(fields: [Type; N]) -> Self {
        Self {
            fields: fields.map(UnnamedField::new).into(),
        }
    }
}

impl Display for UnnamedFields {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        "{".fmt(f)?;
        for (i, field) in self.fields.iter().enumerate() {
            if i > 0 {
                ", ".fmt(f)?;
            }
            field.fmt(f)?;
        }
        "}".fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnnamedField {
    pub t: Type,
}

impl UnnamedField {
    pub fn new(t: Type) -> Self {
        Self { t }
    }

    pub fn name(self, n: impl Into<String>) -> NamedField {
        NamedField {
            n: n.into(),
            t: self.t,
        }
    }
}

impl Display for UnnamedField {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.t.fmt(f)
    }
}

#[derive(Clone, Debug, Eq)]
pub enum Fields {
    Named(NamedFields),
    Unnamed(UnnamedFields),
}

impl Fields {
    pub fn named(fields: impl Into<NamedFields>) -> Self {
        Self::Named(fields.into())
    }

    pub fn unnamed(fields: impl Into<UnnamedFields>) -> Self {
        Self::Unnamed(fields.into())
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match self {
            Self::Unnamed(unnamed) => unnamed.is_compat(value),
            Self::Named(named) => named.is_compat(value),
        }
    }

    pub fn as_named(&self) -> Option<&NamedFields> {
        match self {
            Self::Named(named) => Some(named),
            _ => None,
        }
    }

    pub fn as_unnamed(&self) -> Option<&UnnamedFields> {
        match self {
            Self::Unnamed(unnamed) => Some(unnamed),
            _ => None,
        }
    }
}

impl PartialEq for Fields {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Named(a), Self::Named(b)) if a.len() == b.len() => {
                let a = a.iter().map(|f| (&f.n, &f.t)).collect::<HashMap<_, _>>();
                let b = b.iter().map(|f| (&f.n, &f.t)).collect::<HashMap<_, _>>();
                a == b
            }
            (Self::Unnamed(a), Self::Unnamed(b)) => a == b,
            _ => false,
        }
    }
}

impl From<Vec<NamedField>> for Fields {
    fn from(fields: Vec<NamedField>) -> Self {
        Self::Named(fields.into())
    }
}

impl<'a> From<Vec<(&'a str, Type)>> for Fields {
    fn from(fields: Vec<(&'a str, Type)>) -> Self {
        Self::Named(fields.into())
    }
}

impl<const N: usize> From<[NamedField; N]> for Fields {
    fn from(fields: [NamedField; N]) -> Self {
        Self::Named(fields.into())
    }
}

impl<'a, const N: usize> From<[(&'a str, Type); N]> for Fields {
    fn from(fields: [(&'a str, Type); N]) -> Self {
        Self::Named(fields.into())
    }
}

impl From<Vec<UnnamedField>> for Fields {
    fn from(fields: Vec<UnnamedField>) -> Self {
        Self::Unnamed(fields.into())
    }
}

impl From<Vec<Type>> for Fields {
    fn from(fields: Vec<Type>) -> Self {
        Self::Unnamed(fields.into())
    }
}

impl<const N: usize> From<[UnnamedField; N]> for Fields {
    fn from(fields: [UnnamedField; N]) -> Self {
        Self::Unnamed(fields.into())
    }
}

impl<const N: usize> From<[Type; N]> for Fields {
    fn from(fields: [Type; N]) -> Self {
        Self::Unnamed(fields.into())
    }
}

impl Display for Fields {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(fields) => fields.fmt(f),
            Self::Unnamed(fields) => fields.fmt(f),
        }
    }
}
