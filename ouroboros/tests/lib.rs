use ouroboros::{
    transpile::{cpp, python, ts},
    TypeName, UnnamedField,
};
#[cfg(test)]
use ouroboros::{Enum, EnumVariant, Record, Type, TypeInfo, Union, UnionVariant};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, TypeInfo)]
struct Unit;

#[allow(dead_code)]
#[derive(Clone, Debug, TypeInfo)]
struct Unnamed(u32, Vec<u32>, Option<u32>);

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, TypeInfo, Serialize)]
struct Foo {
    /// This is the x field.
    x: u32,
    /// This is another field, called the y field.
    y: Vec<u32>,
    /// And lastly the z field has a lot of documentation so that we can test
    /// multiple lines but also other complex strings.
    ///
    /// # For example.
    ///
    /// Empty newlines and headers need to work. And:
    ///
    /// - so
    /// - do
    /// - lists
    ///
    /// And ideally:
    ///
    /// ```
    /// code blocks should also work.
    /// ```
    ///
    /// And finally:
    ///     We cannot forget about
    ///     Tab support.
    ///
    /// That should be enough!
    z: Option<u32>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, TypeInfo)]
struct Gen<T: TypeInfo>(T);

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, TypeInfo, Serialize)]
enum Bar {
    X,
    #[serde(rename = "Z")]
    Y,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, TypeInfo, Serialize)]
enum Baz {
    X,
    Y(u32, Vec<u32>),
    #[serde(rename = "W")]
    Z {
        foo: Foo,
        bar: Bar,
    },
}

#[test]
fn test_unit_record() {
    assert_eq!(
        Unit::tname(),
        TypeName {
            n: "Unit",
            g: vec![]
        }
    );
    assert_eq!(Unit::t(), Type::Record(Record::new_unit("Unit")));
}

#[test]
fn test_unnamed_record() {
    assert_eq!(
        Unnamed::tname(),
        TypeName {
            n: "Unnamed",
            g: vec![]
        }
    );
    assert_eq!(
        Unnamed::t(),
        Type::Record(Record::new(
            "Unnamed",
            [Type::U32, Vec::<u32>::t(), Option::<u32>::t(),].map(UnnamedField::new)
        ))
    );
}

#[test]
fn test_named_record() {
    assert_eq!(
        Foo::tname(),
        TypeName {
            n: "Foo",
            g: vec![]
        }
    );
    assert_eq!(
        Foo::t(),
        Type::Record(Record::with_doc(
            [("x", "This is the x field."), ("y", "This is another field, called the y field."), ("z", "And lastly the z field has a lot of documentation so that we can test\nmultiple lines but also other complex strings.\n\n# For example.\n\nEmpty newlines and headers need to work. And:\n\n- so\n- do\n- lists\n\nAnd ideally:\n\n```\ncode blocks should also work.\n```\n\nAnd finally:\n    We cannot forget about\n    Tab support.\n\nThat should be enough!")],
            "Foo",
            [
                ("x", Type::U32,),
                (
                    "y",
                    Vec::<u32>::t(),
                ),
                ("z", Option::<u32>::t(),)
            ]
        ))
    );
}

#[test]
fn test_generic_record() {
    assert_eq!(
        Gen::<Foo>::tname(),
        TypeName {
            n: "Gen",
            g: vec![Foo::tname()]
        }
    );
    assert_eq!(
        Gen::<Foo>::t(),
        Type::Record(Record::new("Gen", [Foo::t()]))
    );
}

#[test]
fn test_enum() {
    assert_eq!(
        Bar::tname(),
        TypeName {
            n: "Bar",
            g: vec![]
        }
    );
    assert_ne!(
        Bar::t(),
        Type::Enum(Enum::new(
            "Bar",
            [EnumVariant::new("X"), EnumVariant::new("Y"),]
        ))
    );
    assert_eq!(
        Bar::t(),
        Type::Enum(Enum::new(
            "Bar",
            [EnumVariant::new("X"), EnumVariant::new("Z"),]
        ))
    );
}

#[test]
fn test_union() {
    assert_eq!(
        Baz::tname(),
        TypeName {
            n: "Baz",
            g: vec![]
        }
    );
    assert_ne!(
        Baz::t(),
        Type::Union(Union::new(
            "Baz",
            [
                UnionVariant::new("X"),
                UnionVariant::with_fields("Y", [u32::t(), Vec::<u32>::t()]),
                UnionVariant::with_fields("Z", [("foo", Foo::t()), ("bar", Bar::t())])
            ]
        ))
    );
    assert_eq!(
        Baz::t(),
        Type::Union(Union::new(
            "Baz",
            [
                UnionVariant::new("X"),
                UnionVariant::with_fields("Y", [u32::t(), Vec::<u32>::t()]),
                UnionVariant::with_fields("W", [("foo", Foo::t()), ("bar", Bar::t())])
            ]
        ))
    );

    println!("{}", &Baz::t());
    println!("{}", serde_json::to_string_pretty(&Baz::t()).unwrap());
    println!("{}", cpp::TypedefVisitor::visit_type(&Baz::t()));
    println!("{}", python::TypedefVisitor::visit_type(&Baz::t()));
    println!("{}", ts::TypedefVisitor::visit_type(&Baz::t()));
}
