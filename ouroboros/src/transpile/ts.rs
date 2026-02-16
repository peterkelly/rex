use crate::{Array, Enum, Fields, Optional, Record, Symbolic, Tuple, Type, Union};

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

    pub fn visit_unit() -> String {
        Self::visit_unit_with_prefix("")
    }

    pub fn visit_unit_with_prefix(prefix: &str) -> String {
        format!("{}{{}}", prefix)
    }

    pub fn visit_bool() -> String {
        Self::visit_bool_with_prefix("")
    }

    pub fn visit_bool_with_prefix(prefix: &str) -> String {
        format!("{}boolean", prefix)
    }

    pub fn visit_i8() -> String {
        Self::visit_i8_with_prefix("")
    }

    pub fn visit_i8_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_i16() -> String {
        Self::visit_i16_with_prefix("")
    }

    pub fn visit_i16_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_i32() -> String {
        Self::visit_i32_with_prefix("")
    }

    pub fn visit_i32_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_i64() -> String {
        Self::visit_i64_with_prefix("")
    }

    pub fn visit_i64_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_i128() -> String {
        Self::visit_i128_with_prefix("")
    }

    pub fn visit_i128_with_prefix(prefix: &str) -> String {
        format!("{}bigint", prefix)
    }

    pub fn visit_u8() -> String {
        Self::visit_u8_with_prefix("")
    }

    pub fn visit_u8_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_u16() -> String {
        Self::visit_u16_with_prefix("")
    }

    pub fn visit_u16_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_u32() -> String {
        Self::visit_u32_with_prefix("")
    }

    pub fn visit_u32_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_u64() -> String {
        Self::visit_u64_with_prefix("")
    }

    pub fn visit_u64_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_u128() -> String {
        Self::visit_u128_with_prefix("")
    }

    pub fn visit_u128_with_prefix(prefix: &str) -> String {
        format!("{}bigint", prefix)
    }

    pub fn visit_f32() -> String {
        Self::visit_f32_with_prefix("")
    }

    pub fn visit_f32_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_f64() -> String {
        Self::visit_f64_with_prefix("")
    }

    pub fn visit_f64_with_prefix(prefix: &str) -> String {
        format!("{}number", prefix)
    }

    pub fn visit_string() -> String {
        Self::visit_string_with_prefix("")
    }

    pub fn visit_string_with_prefix(prefix: &str) -> String {
        format!("{}string", prefix)
    }

    pub fn visit_array(arr: &Array) -> String {
        Self::visit_array_with_prefix(arr, "")
    }

    pub fn visit_array_with_prefix(arr: &Array, prefix: &str) -> String {
        format!("{}Array<{}>", prefix, &TypenameVisitor::visit_type(&arr.t))
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
            "{}[{}]",
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
        format!("{}({} | null)", prefix, Self::visit_type(&optional.t))
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

    pub fn visit_unit_with_prefix(prefix: &str) -> String {
        TypenameVisitor::visit_unit_with_prefix(prefix)
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
                s.push_str(" = [");
                for (i, field) in fields.iter().enumerate() {
                    s.push_str(&TypenameVisitor::visit_type(&field.t));
                    if i != fields.len() - 1 {
                        s.push_str(", ");
                    }
                }
                s.push_str("];");
            }
            Fields::Named(fields) => {
                s.push_str("type ");
                s.push_str(&record.n);
                s.push_str(" = {\n");
                for field in fields.iter() {
                    s.push_str(prefix);
                    s.push_str("    ");
                    s.push_str(&TypenameVisitor::visit_type(&field.t));
                    s.push(' ');
                    s.push_str(&field.n);
                    s.push_str(";\n");
                }
                s.push_str(prefix);
                s.push_str("};");
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
        s.push_str("enum ");
        s.push_str(&e.n);
        s.push_str(" {\n");
        for variant in &e.variants {
            s.push_str(prefix);
            s.push_str("    ");
            s.push_str(&variant.n);
            if let Some(v) = &variant.v {
                s.push_str(" = ");
                s.push_str(&v.to_string());
            }
            s.push_str(",\n");
        }
        s.push_str(prefix);
        s.push('}');
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

        for variant in &union.variants {
            match &variant.fields {
                Some(Fields::Unnamed(fields)) => {
                    s.push_str(&Self::visit_record_with_prefix(
                        &Record::new(
                            format!("{}_{}", &union.n, &variant.n),
                            Fields::Unnamed(fields.clone()),
                        ),
                        prefix,
                    ));
                    s.push('\n');
                }
                Some(Fields::Named(fields)) => {
                    s.push_str(&Self::visit_record_with_prefix(
                        &Record::new(
                            format!("{}_{}", &union.n, &variant.n),
                            Fields::named(fields.clone()),
                        ),
                        prefix,
                    ));
                    s.push('\n');
                }
                None => {
                    s.push_str(prefix);
                    s.push_str(&union.n);
                    s.push('_');
                    s.push_str(&variant.n);
                    s.push_str(" = {};\n");
                }
            }
        }

        s.push_str(prefix);
        s.push_str(&union.n);
        s.push_str(" = ");
        for (i, variant) in union.variants.iter().enumerate() {
            s.push_str(&variant.n);
            if i < union.variants.len() - 1 {
                s.push_str(" | ");
            }
        }
        s.push(';');

        s
    }

    pub fn visit_symbolic(sym: &Symbolic) -> String {
        Self::visit_symbolic_with_prefix(sym, "")
    }

    pub fn visit_symbolic_with_prefix(sym: &Symbolic, prefix: &str) -> String {
        let mut s = String::new();
        s.push_str(prefix);
        s.push_str("type ");
        s.push_str(&sym.n);
        s.push_str("_Symbol = \"");
        s.push_str(&sym.n);
        s.push_str("\";");
        s
    }
}
