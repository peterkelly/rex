use crate::{
    field::Fields,
    product::{Array, Record, Tuple},
    sum::{Enum, EnumVariant, Optional, Union},
    symbolic::Symbolic,
    type_info::Type,
    NamedFields,
};

pub struct TypenameVisitor;

impl TypenameVisitor {
    pub fn visit_type(t: &Type) -> String {
        Self::visit_type_with_prefix(t, "")
    }

    pub fn visit_type_with_prefix(t: &Type, prefix: &str) -> String {
        match t {
            Type::Unit => todo!(),
            Type::Bool => Self::visit_bool_with_prefix(prefix),
            Type::I8 => Self::visit_i8_with_prefix(prefix),
            Type::I16 => Self::visit_i16_with_prefix(prefix),
            Type::I32 => Self::visit_i32_with_prefix(prefix),
            Type::I64 => Self::visit_i64_with_prefix(prefix),
            Type::I128 => Self::visit_i128_with_prefix(prefix),
            Type::U8 => Self::visit_u8_with_prefix(prefix),
            Type::U16 => Self::visit_u16_with_prefix(prefix),
            Type::U32 => Self::visit_u32_with_prefix(prefix),
            Type::U64 => Self::visit_u64_with_prefix(prefix),
            Type::U128 => Self::visit_u128_with_prefix(prefix),
            Type::F32 => Self::visit_f32_with_prefix(prefix),
            Type::F64 => Self::visit_f64_with_prefix(prefix),
            Type::String => Self::visit_string_with_prefix(prefix),
            Type::Array(arr) => Self::visit_array_with_prefix(arr, prefix),
            Type::Func(_) => todo!(),
            Type::Record(record) => Self::visit_record_with_prefix(record, prefix),
            Type::Tuple(tuple) => Self::visit_tuple_with_prefix(tuple, prefix),
            Type::Enum(e) => Self::visit_enum_with_prefix(e, prefix),
            Type::Fallible(_) => unimplemented!(),
            Type::Optional(optional) => Self::visit_optional_with_prefix(optional, prefix),
            Type::Union(union) => Self::visit_union_with_prefix(union, prefix),
            Type::Ptr(_) => todo!(),
            Type::Symbolic(sym) => Self::visit_symbolic_with_prefix(sym, prefix),
            Type::Generic(_) => todo!(),
            Type::Alias(alias) => Self::visit_type(&alias.t), // FIXME: Should we transpile the alias?
        }
    }

    pub fn visit_bool() -> String {
        Self::visit_bool_with_prefix("")
    }

    pub fn visit_bool_with_prefix(prefix: &str) -> String {
        format!("{}bool", prefix)
    }

    pub fn visit_i8() -> String {
        Self::visit_i8_with_prefix("")
    }

    pub fn visit_i8_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_i16() -> String {
        Self::visit_i16_with_prefix("")
    }

    pub fn visit_i16_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_i32() -> String {
        Self::visit_i32_with_prefix("")
    }

    pub fn visit_i32_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_i64() -> String {
        Self::visit_i64_with_prefix("")
    }

    pub fn visit_i64_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_i128() -> String {
        Self::visit_i128_with_prefix("")
    }

    pub fn visit_i128_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_u8() -> String {
        Self::visit_u8_with_prefix("")
    }

    pub fn visit_u8_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_u16() -> String {
        Self::visit_u16_with_prefix("")
    }

    pub fn visit_u16_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_u32() -> String {
        Self::visit_u32_with_prefix("")
    }

    pub fn visit_u32_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_u64() -> String {
        Self::visit_u64_with_prefix("")
    }

    pub fn visit_u64_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_u128() -> String {
        Self::visit_u128_with_prefix("")
    }

    pub fn visit_u128_with_prefix(prefix: &str) -> String {
        format!("{}int", prefix)
    }

    pub fn visit_f32() -> String {
        Self::visit_f32_with_prefix("")
    }

    pub fn visit_f32_with_prefix(prefix: &str) -> String {
        format!("{}float", prefix)
    }

    pub fn visit_f64() -> String {
        Self::visit_f64_with_prefix("")
    }

    pub fn visit_f64_with_prefix(prefix: &str) -> String {
        format!("{}float", prefix)
    }

    pub fn visit_string() -> String {
        Self::visit_string_with_prefix("")
    }

    pub fn visit_string_with_prefix(prefix: &str) -> String {
        format!("{}str", prefix)
    }

    pub fn visit_array(arr: &Array) -> String {
        Self::visit_array_with_prefix(arr, "")
    }

    pub fn visit_array_with_prefix(arr: &Array, prefix: &str) -> String {
        format!("{}list[{}]", prefix, &TypenameVisitor::visit_type(&arr.t))
    }

    pub fn visit_record(record: &Record) -> String {
        Self::visit_record_with_prefix(record, "")
    }

    pub fn visit_record_with_prefix(record: &Record, prefix: &str) -> String {
        format!("{}{}", prefix, &record.n)
    }

    pub fn visit_tuple(tuple: &Tuple) -> String {
        Self::visit_tuple_with_prefix(tuple, "")
    }

    pub fn visit_tuple_with_prefix(tuple: &Tuple, prefix: &str) -> String {
        format!(
            "{}tuple[{}]",
            prefix,
            tuple
                .fields
                .iter()
                .map(|f| Self::visit_type(&f.t))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    pub fn visit_enum(e: &Enum) -> String {
        Self::visit_enum_with_prefix(e, "")
    }

    pub fn visit_enum_with_prefix(e: &Enum, prefix: &str) -> String {
        format!("{}{}", prefix, &e.n)
    }

    pub fn visit_optional(optional: &Optional) -> String {
        Self::visit_optional_with_prefix(optional, "")
    }

    pub fn visit_optional_with_prefix(optional: &Optional, prefix: &str) -> String {
        format!("{}Optional[{}]", prefix, Self::visit_type(&optional.t))
    }

    pub fn visit_union(union: &Union) -> String {
        Self::visit_union_with_prefix(union, "")
    }

    pub fn visit_union_with_prefix(union: &Union, prefix: &str) -> String {
        format!("{}{}", prefix, &union.n)
    }

    pub fn visit_symbolic(sym: &Symbolic) -> String {
        Self::visit_symbolic_with_prefix(sym, "")
    }

    pub fn visit_symbolic_with_prefix(sym: &Symbolic, prefix: &str) -> String {
        format!("{}{}_Symbol", prefix, &sym.n)
    }
}

pub struct TypedefVisitor;

impl TypedefVisitor {
    pub fn visit_type(t: &Type) -> String {
        Self::visit_type_with_prefix(t, "")
    }

    pub fn visit_type_with_prefix(t: &Type, prefix: &str) -> String {
        match t {
            Type::Unit => todo!(),
            Type::Bool => Self::visit_bool_with_prefix(prefix),
            Type::I8 => Self::visit_i8_with_prefix(prefix),
            Type::I16 => Self::visit_i16_with_prefix(prefix),
            Type::I32 => Self::visit_i32_with_prefix(prefix),
            Type::I64 => Self::visit_i64_with_prefix(prefix),
            Type::I128 => Self::visit_i128_with_prefix(prefix),
            Type::U8 => Self::visit_u8_with_prefix(prefix),
            Type::U16 => Self::visit_u16_with_prefix(prefix),
            Type::U32 => Self::visit_u32_with_prefix(prefix),
            Type::U64 => Self::visit_u64_with_prefix(prefix),
            Type::U128 => Self::visit_u128_with_prefix(prefix),
            Type::F32 => Self::visit_f32_with_prefix(prefix),
            Type::F64 => Self::visit_f64_with_prefix(prefix),
            Type::String => Self::visit_string_with_prefix(prefix),
            Type::Array(arr) => Self::visit_array_with_prefix(arr, prefix),
            Type::Func(_) => todo!(),
            Type::Record(record) => Self::visit_record_with_prefix(record, prefix),
            Type::Tuple(tuple) => Self::visit_tuple_with_prefix(tuple, prefix),
            Type::Enum(e) => Self::visit_enum_with_prefix(e, prefix),
            Type::Fallible(_) => unimplemented!(),
            Type::Optional(optional) => Self::visit_optional_with_prefix(optional, prefix),
            Type::Union(union) => Self::visit_union_with_prefix(union, prefix),
            Type::Ptr(_) => todo!(),
            Type::Symbolic(sym) => Self::visit_symbolic_with_prefix(sym, prefix),
            Type::Generic(_) => todo!(),
            Type::Alias(alias) => Self::visit_type(&alias.t), // FIXME: Should we transpile the alias?
        }
    }

    pub fn visit_bool() -> String {
        Self::visit_bool_with_prefix("")
    }

    pub fn visit_bool_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_bool_with_prefix(prefix)
    }

    pub fn visit_i8() -> String {
        Self::visit_i8_with_prefix("")
    }

    pub fn visit_i8_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_i8_with_prefix(prefix)
    }

    pub fn visit_i16() -> String {
        Self::visit_i16_with_prefix("")
    }

    pub fn visit_i16_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_i16_with_prefix(prefix)
    }

    pub fn visit_i32() -> String {
        Self::visit_i32_with_prefix("")
    }

    pub fn visit_i32_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_i32_with_prefix(prefix)
    }

    pub fn visit_i64() -> String {
        Self::visit_i64_with_prefix("")
    }

    pub fn visit_i64_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_i64_with_prefix(prefix)
    }

    pub fn visit_i128() -> String {
        Self::visit_i128_with_prefix("")
    }

    pub fn visit_i128_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_i128_with_prefix(prefix)
    }

    pub fn visit_u8() -> String {
        Self::visit_u8_with_prefix("")
    }

    pub fn visit_u8_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_u8_with_prefix(prefix)
    }

    pub fn visit_u16() -> String {
        Self::visit_u16_with_prefix("")
    }

    pub fn visit_u16_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_u16_with_prefix(prefix)
    }

    pub fn visit_u32() -> String {
        Self::visit_u32_with_prefix("")
    }

    pub fn visit_u32_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_u32_with_prefix(prefix)
    }

    pub fn visit_u64() -> String {
        Self::visit_u64_with_prefix("")
    }

    pub fn visit_u64_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_u64_with_prefix(prefix)
    }

    pub fn visit_u128() -> String {
        Self::visit_u128_with_prefix("")
    }

    pub fn visit_u128_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_u128_with_prefix(prefix)
    }

    pub fn visit_f32() -> String {
        Self::visit_f32_with_prefix("")
    }

    pub fn visit_f32_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_f32_with_prefix(prefix)
    }

    pub fn visit_f64() -> String {
        Self::visit_f64_with_prefix("")
    }

    pub fn visit_f64_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_f64_with_prefix(prefix)
    }

    pub fn visit_string() -> String {
        Self::visit_string_with_prefix("")
    }

    pub fn visit_string_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_string_with_prefix(prefix)
    }

    pub fn visit_array(list: &Array) -> String {
        Self::visit_array_with_prefix(list, "")
    }

    pub fn visit_array_with_prefix(list: &Array, prefix: &str) -> String {
        TypenameVisitor::visit_array_with_prefix(list, prefix)
    }

    pub fn visit_record(record: &Record) -> String {
        Self::visit_record_with_prefix(record, "")
    }

    pub fn visit_record_with_prefix(record: &Record, prefix: &str) -> String {
        let mut s = String::new();
        s.push_str(prefix);
        match &record.fields {
            Fields::Unnamed(fields) => {
                s.push_str("type ");
                s.push_str(&record.n);
                s.push_str(" = tuple[");
                for (i, field) in fields.iter().enumerate() {
                    s.push_str(&TypenameVisitor::visit_type(&field.t));
                    if i != fields.len() - 1 {
                        s.push_str(", ");
                    }
                }
                s.push(']');
            }
            Fields::Named(fields) => {
                if !fields.is_empty() {
                    s.push_str("@dataclass\n");
                    s.push_str(prefix);
                }
                s.push_str("class ");
                s.push_str(&record.n);
                s.push_str(":\n");
                for (i, field) in fields.iter().enumerate() {
                    s.push_str(prefix);
                    s.push_str("    ");
                    s.push_str(&field.n);
                    s.push_str(": ");
                    s.push_str(&TypenameVisitor::visit_type(&field.t));
                    if i != fields.len() - 1 {
                        s.push('\n');
                    }
                }
                if fields.is_empty() {
                    s.push_str(prefix);
                    s.push_str("    pass");
                }
            }
        }
        s
    }

    pub fn visit_tuple(tuple: &Tuple) -> String {
        Self::visit_tuple_with_prefix(tuple, "")
    }

    pub fn visit_tuple_with_prefix(tuple: &Tuple, prefix: &str) -> String {
        TypenameVisitor::visit_tuple_with_prefix(tuple, prefix)
    }

    pub fn visit_enum(e: &Enum) -> String {
        Self::visit_enum_with_prefix(e, "")
    }

    pub fn visit_enum_with_prefix(e: &Enum, prefix: &str) -> String {
        let mut s = String::new();
        s.push_str(prefix);
        s.push_str("class ");
        s.push_str(&e.n);
        s.push_str("(Enum):\n");
        for (i, variant) in e.variants.iter().enumerate() {
            s.push_str(prefix);
            s.push_str("    ");
            s.push_str(&variant.n);
            if let Some(v) = &variant.v {
                s.push_str(" = ");
                s.push_str(&v.to_string());
            }
            if i != e.variants.len() - 1 {
                s.push('\n');
            }
        }
        s
    }

    pub fn visit_optional(optional: &Optional) -> String {
        Self::visit_optional_with_prefix(optional, "")
    }

    pub fn visit_optional_with_prefix(optional: &Optional, prefix: &str) -> String {
        TypenameVisitor::visit_optional_with_prefix(optional, prefix)
    }

    pub fn visit_union(union: &Union) -> String {
        Self::visit_union_with_prefix(union, "")
    }

    pub fn visit_union_with_prefix(union: &Union, prefix: &str) -> String {
        let mut s = String::new();
        s.push_str(prefix);
        s.push_str("@dataclass\n");
        s.push_str(prefix);
        s.push_str("class ");
        s.push_str(&union.n);
        s.push_str(":\n");

        s.push_str(&Self::visit_enum_with_prefix(
            &Enum::new(
                "Tag",
                union
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(i, variant)| EnumVariant::with_const_value(variant.n.clone(), i as u8))
                    .collect::<Vec<_>>(),
            ),
            &format!("{}    ", prefix),
        ));
        s.push_str("\n\n");

        for variant in &union.variants {
            match &variant.fields {
                Some(Fields::Unnamed(fields)) => {
                    s.push_str(&Self::visit_record_with_prefix(
                        &Record::new(
                            format!("{}_Body", &variant.n),
                            Fields::Unnamed(fields.clone()),
                        ),
                        &format!("{}    ", prefix),
                    ));
                    s.push('\n');
                }
                Some(Fields::Named(fields)) => {
                    s.push_str(&Self::visit_record_with_prefix(
                        &Record::new(
                            format!("{}_Body", &variant.n),
                            Fields::named(fields.clone()),
                        ),
                        &format!("{}    ", prefix),
                    ));
                    s.push('\n');
                }
                None => {
                    s.push_str(&Self::visit_record_with_prefix(
                        &Record::new(
                            format!("{}_Body", &variant.n),
                            Fields::named(NamedFields::empty()),
                        ),
                        &format!("{}    ", prefix),
                    ));
                    s.push('\n');
                }
            }
        }
        s.push('\n');
        s.push_str(prefix);
        s.push_str("    tag: Tag\n");
        s.push_str(prefix);
        s.push_str("    data: Union[");
        for (i, variant) in union.variants.iter().enumerate() {
            s.push_str(&format!("{}_Body", &variant.n));
            if i != union.variants.len() - 1 {
                s.push_str(", ");
            }
        }
        s.push(']');
        s
    }

    pub fn visit_symbolic(sym: &Symbolic) -> String {
        Self::visit_symbolic_with_prefix(sym, "")
    }

    pub fn visit_symbolic_with_prefix(sym: &Symbolic, prefix: &str) -> String {
        let mut s = String::new();
        s.push_str(prefix);
        s.push_str("class ");
        s.push_str(&sym.n);
        s.push_str("_Symbol:\n");
        s.push_str(prefix);
        s.push_str("    pass");
        s
    }
}

#[cfg(test)]
mod test {
    use crate::{
        field::{NamedField, UnnamedField},
        product::{Array, Record, Tuple},
        sum::{Enum, EnumVariant, Optional, Union, UnionVariant},
        symbolic::Symbolic,
        type_info::Type,
    };

    use super::TypedefVisitor;

    #[test]
    fn simple_array() {
        let t = Array::new(Type::U8);

        assert_eq!(TypedefVisitor::visit_array(&t), r#"list[int]"#);
    }

    #[test]
    fn simple_record() {
        let t = Record::new(
            "Foo",
            [
                NamedField::new("bar", Type::U8),
                NamedField::new("baz", Type::U8),
            ],
        );

        assert_eq!(
            TypedefVisitor::visit_record(&t),
            r#"@dataclass
class Foo:
    bar: int
    baz: int"#
        );

        let t = Record::new(
            "Foo",
            [UnnamedField::new(Type::U8), UnnamedField::new(Type::U8)],
        );

        assert_eq!(
            TypedefVisitor::visit_record(&t),
            r#"type Foo = tuple[int, int]"#
        );
    }

    #[test]
    fn simple_tuple() {
        let t = Type::from(Tuple::new([
            UnnamedField::new(Type::U8),
            UnnamedField::new(Type::U8),
        ]));

        assert_eq!(TypedefVisitor::visit_type(&t), r#"tuple[int, int]"#);
    }

    #[test]
    fn simple_enum() {
        let t = Enum::new(
            "Foo",
            [
                EnumVariant::with_const_value("Bar", 0),
                EnumVariant::with_const_value("Baz", 1),
            ],
        );

        assert_eq!(
            TypedefVisitor::visit_enum(&t),
            r#"class Foo(Enum):
    Bar = 0
    Baz = 1"#
        );
    }

    #[test]
    fn simple_optional() {
        let t = Optional::new(Type::U8);

        assert_eq!(TypedefVisitor::visit_optional(&t), r#"Optional[int]"#);
    }

    #[test]
    fn simple_union() {
        let t = Union::new(
            "Foo",
            [
                UnionVariant::with_fields(
                    "X",
                    [UnnamedField::new(Type::U8), UnnamedField::new(Type::U8)],
                ),
                UnionVariant::with_fields(
                    "Y",
                    [
                        NamedField::new("bar", Type::U8),
                        NamedField::new("baz", Type::U8),
                    ],
                ),
                UnionVariant::new("Z"),
            ],
        );

        assert_eq!(
            TypedefVisitor::visit_union(&t),
            r#"@dataclass
class Foo:
    class Tag(Enum):
        X = 0
        Y = 1
        Z = 2

    type X_Body = tuple[int, int]
    @dataclass
    class Y_Body:
        bar: int
        baz: int
    class Z_Body:
        pass

    tag: Tag
    data: Union[X_Body, Y_Body, Z_Body]"#
        );
    }

    #[test]
    fn simple_symbolic() {
        let t = Symbolic::new("Foo");

        assert_eq!(
            TypedefVisitor::visit_symbolic(&t),
            r#"class Foo_Symbol:
    pass"#
        );
    }
}
