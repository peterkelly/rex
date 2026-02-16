use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

use crate::{Func, Type, TypeInfo, TypeName};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Lambda<A, B> {
    pub doc: Option<String>,
    #[serde(rename = "λ")]
    pub n: String,
    pub extras: serde_json::Value,
    pub captured_args: Vec<serde_json::Value>,

    #[serde(skip)]
    pub _args: PhantomData<A>,
    #[serde(skip)]
    pub _ret: PhantomData<B>,
}

impl<A, B> Lambda<A, B> {
    pub fn new(n: impl Into<String>) -> Self {
        Self {
            doc: None,
            n: n.into(),
            extras: serde_json::Value::Null,
            captured_args: vec![],

            _args: PhantomData,
            _ret: PhantomData,
        }
    }

    pub fn with_extras(n: impl Into<String>, extras: serde_json::Value) -> Self {
        Self {
            doc: None,
            n: n.into(),
            extras,
            captured_args: vec![],

            _args: PhantomData,
            _ret: PhantomData,
        }
    }

    pub fn with_captured_args(n: impl Into<String>, captured_args: Vec<serde_json::Value>) -> Self {
        Self {
            doc: None,
            n: n.into(),
            extras: serde_json::Value::Null,
            captured_args,

            _args: PhantomData,
            _ret: PhantomData,
        }
    }

    pub fn with_docs(n: impl Into<String>, doc: impl Into<String>) -> Self {
        Self {
            doc: Some(doc.into()),
            n: n.into(),
            extras: serde_json::Value::Null,
            captured_args: vec![],

            _args: PhantomData,
            _ret: PhantomData,
        }
    }
}

impl<A, B> Lambda<A, B>
where
    A: TypeInfo,
    B: TypeInfo,
{
    pub fn type_info(&self) -> Type {
        <Lambda<A, B> as TypeInfo>::t()
    }
}

impl<A, B> TypeInfo for Lambda<A, B>
where
    A: TypeInfo,
    B: TypeInfo,
{
    fn tname() -> TypeName {
        TypeName {
            n: "λ",
            g: vec![A::tname(), B::tname()],
        }
    }

    fn t() -> Type {
        Type::from(Func::new(A::t(), B::t()))
    }
}

pub trait Curry {
    type Arg: Serialize;
    type Closure;
    type Error;

    fn curry(self, arg: Self::Arg) -> Result<Self::Closure, Self::Error>;
}

macro_rules! impl_curry {
    ($($arg: ident),*) => {
        impl<A0, $($arg),*, B> Curry for Lambda<(A0, $($arg),*), B>
        where
            A0: Serialize,
        {
            type Arg = A0;
            #[allow(unused_parens)] // macro_rules is too stupid to realise that the parens are in fact necessary to ensure that the defined type is a typle
            type Closure = Lambda<($($arg),*), B>;
            type Error = serde_json::Error;

            fn curry(self, arg: Self::Arg) -> Result<Self::Closure, Self::Error> {
                let mut captured_args = self.captured_args;
                captured_args.push(serde_json::to_value(&arg)?);
                Ok(Lambda {
                    doc: self.doc,
                    n: self.n,
                    extras: self.extras,
                    captured_args,

                    _args: PhantomData,
                    _ret: PhantomData,
                })
            }
        }
    };
}

impl<A0, A1, B> Curry for Lambda<(A0, A1), B>
where
    A0: Serialize,
{
    type Arg = A0;
    type Closure = Lambda<A1, B>;
    type Error = serde_json::Error;

    fn curry(self, arg: Self::Arg) -> Result<Self::Closure, Self::Error> {
        let mut captured_args = self.captured_args;
        captured_args.push(serde_json::to_value(&arg)?);
        Ok(Lambda {
            doc: self.doc,
            n: self.n,
            extras: self.extras,
            captured_args,

            _args: PhantomData,
            _ret: PhantomData,
        })
    }
}

// Explicitly implement the 2-tuple case so that we do not end up with a 1-tuple after currying.

impl_curry!(A1, A2);
impl_curry!(A1, A2, A3);
impl_curry!(A1, A2, A3, A4);
impl_curry!(A1, A2, A3, A4, A5);
impl_curry!(A1, A2, A3, A4, A5, A6);
impl_curry!(A1, A2, A3, A4, A5, A6, A7);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8, A9);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, A13);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, A13, A14);
impl_curry!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, A13, A14, A15);
