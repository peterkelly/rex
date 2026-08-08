use std::{collections::BTreeMap, fmt, sync::Arc};

use blake3::Hash;
use chrono::{DateTime, Utc};
use rex_ast::{Symbol, char_literal};
use rex_typesystem::{
    types::{AdtDecl, Type, TypeKind},
    typesystem::TypeSystem,
};
use uuid::Uuid;

use crate::{
    EngineError,
    memory::{
        heap::{RootScope, RootedPtr, ValueDisplayOptions},
        traits::FromRex,
    },
};

/// Owned semantic data exchanged with Rex embedders.
///
/// `Value` contains no heap references or evaluator-only states. Composite
/// variants recursively own their children, so a value may be moved to another
/// task without retaining access to the Rex heap.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),
    Uuid(Uuid),
    Hash(Hash),
    DateTime(DateTime<Utc>),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Bytes(Vec<u8>),
    Dict(BTreeMap<String, Value>),
    Adt(Symbol, Vec<Value>),
}

impl Value {
    pub fn value_type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "Bool",
            Self::U8(_) => "u8",
            Self::U16(_) => "u16",
            Self::U32(_) => "u32",
            Self::U64(_) => "u64",
            Self::I8(_) => "i8",
            Self::I16(_) => "i16",
            Self::I32(_) => "i32",
            Self::I64(_) => "i64",
            Self::F32(_) => "f32",
            Self::F64(_) => "f64",
            Self::Char(_) => "Char",
            Self::String(_) => "String",
            Self::Uuid(_) => "UUID",
            Self::Hash(_) => "Hash",
            Self::DateTime(_) => "DateTime",
            Self::Tuple(_) => "tuple",
            Self::List(_) => "list",
            Self::Bytes(_) => "bytes",
            Self::Dict(_) => "dict",
            Self::Adt(_, _) => "adt",
        }
    }

    pub fn display(&self) -> Result<String, EngineError> {
        self.display_with(ValueDisplayOptions::default())
    }

    pub fn display_with(&self, options: ValueDisplayOptions) -> Result<String, EngineError> {
        Ok(self.render(options))
    }

    pub fn to_rust<T: FromRex>(&self) -> Result<T, EngineError> {
        T::from_rex(self.clone())
    }

    pub fn as_bool(&self) -> Result<bool, EngineError> {
        self.to_rust()
    }

    pub fn as_u8(&self) -> Result<u8, EngineError> {
        self.to_rust()
    }

    pub fn as_u16(&self) -> Result<u16, EngineError> {
        self.to_rust()
    }

    pub fn as_u32(&self) -> Result<u32, EngineError> {
        self.to_rust()
    }

    pub fn as_u64(&self) -> Result<u64, EngineError> {
        self.to_rust()
    }

    pub fn as_i8(&self) -> Result<i8, EngineError> {
        self.to_rust()
    }

    pub fn as_i16(&self) -> Result<i16, EngineError> {
        self.to_rust()
    }

    pub fn as_i32(&self) -> Result<i32, EngineError> {
        self.to_rust()
    }

    pub fn as_i64(&self) -> Result<i64, EngineError> {
        self.to_rust()
    }

    pub fn as_f32(&self) -> Result<f32, EngineError> {
        self.to_rust()
    }

    pub fn as_f64(&self) -> Result<f64, EngineError> {
        self.to_rust()
    }

    pub fn as_char(&self) -> Result<char, EngineError> {
        self.to_rust()
    }

    pub fn as_string(&self) -> Result<String, EngineError> {
        self.to_rust()
    }

    pub fn as_tuple(&self) -> Result<&[Value], EngineError> {
        match self {
            Self::Tuple(items) => Ok(items),
            other => Err(EngineError::NativeType {
                expected: "tuple".into(),
                got: other.value_type_name().into(),
            }),
        }
    }

    pub fn as_list(&self) -> Result<&[Value], EngineError> {
        match self {
            Self::List(items) => Ok(items),
            other => Err(EngineError::NativeType {
                expected: "list".into(),
                got: other.value_type_name().into(),
            }),
        }
    }

    pub fn as_bytes(&self) -> Result<&[u8], EngineError> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            other => Err(EngineError::NativeType {
                expected: "bytes".into(),
                got: other.value_type_name().into(),
            }),
        }
    }

    pub fn as_dict(&self) -> Result<&BTreeMap<String, Value>, EngineError> {
        match self {
            Self::Dict(fields) => Ok(fields),
            other => Err(EngineError::NativeType {
                expected: "dict".into(),
                got: other.value_type_name().into(),
            }),
        }
    }

    fn render(&self, options: ValueDisplayOptions) -> String {
        macro_rules! number {
            ($value:expr, $suffix:literal) => {
                if options.include_numeric_suffixes {
                    format!("{}{}", $value, $suffix)
                } else {
                    $value.to_string()
                }
            };
        }
        match self {
            Self::Bool(value) => value.to_string(),
            Self::U8(value) => number!(value, "u8"),
            Self::U16(value) => number!(value, "u16"),
            Self::U32(value) => number!(value, "u32"),
            Self::U64(value) => number!(value, "u64"),
            Self::I8(value) => number!(value, "i8"),
            Self::I16(value) => number!(value, "i16"),
            Self::I32(value) => number!(value, "i32"),
            Self::I64(value) => number!(value, "i64"),
            Self::F32(value) => number!(value, "f32"),
            Self::F64(value) => number!(value, "f64"),
            Self::Char(value) => char_literal(*value),
            Self::String(value) => format!("{value:?}"),
            Self::Uuid(value) => value.to_string(),
            Self::Hash(value) => value.to_hex().to_string(),
            Self::DateTime(value) => value.to_string(),
            Self::Tuple(items) => format!(
                "({})",
                items
                    .iter()
                    .map(|item| item.render(options))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(|item| item.render(options))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Bytes(bytes) => format!(
                "[{}]",
                bytes
                    .iter()
                    .map(|byte| {
                        if options.include_numeric_suffixes {
                            format!("{byte}u8")
                        } else {
                            byte.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Dict(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(name, value)| format!("{name} = {}", value.render(options)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Adt(name, fields) => {
                let name = if options.strip_internal_snippet_qualifiers
                    && name.as_ref().starts_with("@snippet")
                {
                    name.as_ref().rsplit('.').next().unwrap_or(name.as_ref())
                } else {
                    name.as_ref()
                };
                let mut rendered = Vec::with_capacity(fields.len() + 1);
                rendered.push(name.to_owned());
                rendered.extend(fields.iter().map(|field| field.render(options)));
                rendered.join(" ")
            }
        }
    }
}

impl RootScope<'_> {
    pub(crate) fn export_value(
        &mut self,
        root: RootedPtr,
        expected: &Type,
        types: &TypeSystem,
    ) -> Result<Value, EngineError> {
        value_from_root(self, root, expected, types, ConversionPath::root())
    }

    pub(crate) fn alloc_value(
        &mut self,
        value: Value,
        expected: &Type,
        types: &TypeSystem,
    ) -> Result<RootedPtr, EngineError> {
        value_into_root(self, value, expected, types, ConversionPath::root())
    }
}

#[derive(Clone)]
struct ConversionPath(Arc<ConversionPathNode>);

enum ConversionPathNode {
    Root,
    Child {
        parent: Option<Arc<ConversionPathNode>>,
        segment: String,
    },
}

impl Drop for ConversionPathNode {
    fn drop(&mut self) {
        let mut current = match self {
            Self::Root => None,
            Self::Child { parent, .. } => parent.take(),
        };
        while let Some(parent) = current {
            match Arc::try_unwrap(parent) {
                Ok(mut node) => {
                    current = match &mut node {
                        Self::Root => None,
                        Self::Child { parent, .. } => parent.take(),
                    };
                }
                Err(shared) => {
                    drop(shared);
                    break;
                }
            }
        }
    }
}

impl ConversionPath {
    fn root() -> Self {
        Self(Arc::new(ConversionPathNode::Root))
    }

    fn child(&self, segment: impl Into<String>) -> Self {
        Self(Arc::new(ConversionPathNode::Child {
            parent: Some(self.0.clone()),
            segment: segment.into(),
        }))
    }
}

impl fmt::Display for ConversionPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut segments = Vec::new();
        let mut current = self.0.as_ref();
        while let ConversionPathNode::Child { parent, segment } = current {
            segments.push(segment.as_str());
            let Some(parent) = parent.as_deref() else {
                break;
            };
            current = parent;
        }
        formatter.write_str("$")?;
        for segment in segments.into_iter().rev() {
            formatter.write_str(segment)?;
        }
        Ok(())
    }
}

enum ExportWork {
    Convert {
        root: RootedPtr,
        expected: Type,
        path: ConversionPath,
    },
    Tuple(usize),
    List(usize),
    Dict(Vec<String>),
    Adt {
        tag: Symbol,
        fields: usize,
    },
}

fn value_from_root(
    scope: &mut RootScope<'_>,
    root: RootedPtr,
    expected: &Type,
    types: &TypeSystem,
    path: ConversionPath,
) -> Result<Value, EngineError> {
    let mut work = vec![ExportWork::Convert {
        root,
        expected: expected.clone(),
        path,
    }];
    let mut values = Vec::new();

    while let Some(item) = work.pop() {
        match item {
            ExportWork::Convert {
                root,
                expected,
                path,
            } => match expected.as_ref() {
                TypeKind::Var(_) => {
                    return Err(conversion_error(
                        &path,
                        &expected,
                        "unresolved type variable",
                    ));
                }
                TypeKind::Fun(_, _) => {
                    return Err(conversion_error(
                        &path,
                        &expected,
                        root_type_name(scope, root),
                    ));
                }
                TypeKind::Tuple(item_types) => {
                    let items = scope.root_as_tuple(root).map_err(|_| {
                        conversion_error(&path, &expected, root_type_name(scope, root))
                    })?;
                    if items.len() != item_types.len() {
                        return Err(conversion_error(
                            &path,
                            &expected,
                            format!("tuple with {} items", items.len()),
                        ));
                    }
                    work.push(ExportWork::Tuple(items.len()));
                    for (index, (item, item_type)) in
                        items.into_iter().zip(item_types).enumerate().rev()
                    {
                        work.push(ExportWork::Convert {
                            root: item,
                            expected: item_type.clone(),
                            path: path.child(format!("[{index}]")),
                        });
                    }
                }
                TypeKind::Record(field_types) => {
                    let fields = scope.root_as_dict(root).map_err(|_| {
                        conversion_error(&path, &expected, root_type_name(scope, root))
                    })?;
                    if fields.len() != field_types.len() {
                        return Err(conversion_error(
                            &path,
                            &expected,
                            format!("record with {} fields", fields.len()),
                        ));
                    }
                    let definitions = field_types.clone();
                    let names = definitions
                        .iter()
                        .map(|(name, _)| name.to_string())
                        .collect::<Vec<_>>();
                    work.push(ExportWork::Dict(names.clone()));
                    for (name, field_type) in definitions.into_iter().rev() {
                        let field = fields.get(name.as_ref()).copied().ok_or_else(|| {
                            conversion_error(
                                path.child(format!(".{name}")),
                                &field_type,
                                "missing field",
                            )
                        })?;
                        work.push(ExportWork::Convert {
                            root: field,
                            expected: field_type,
                            path: path.child(format!(".{name}")),
                        });
                    }
                }
                TypeKind::Con(_) | TypeKind::App(_, _) => {
                    let (head, args) = decompose_type_app(&expected);
                    let TypeKind::Con(con) = head.as_ref() else {
                        return Err(conversion_error(
                            &path,
                            &expected,
                            root_type_name(scope, root),
                        ));
                    };
                    let name = con.name();
                    let mismatch = |scope: &RootScope<'_>| {
                        conversion_error(&path, &expected, root_type_name(scope, root))
                    };
                    let scalar = match (name.as_ref(), args.as_slice()) {
                        ("Bool", []) => Some(
                            scope
                                .root_as_bool(root)
                                .map(Value::Bool)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("u8", []) => Some(
                            scope
                                .root_as_u8(root)
                                .map(Value::U8)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("u16", []) => Some(
                            scope
                                .root_as_u16(root)
                                .map(Value::U16)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("u32", []) => Some(
                            scope
                                .root_as_u32(root)
                                .map(Value::U32)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("u64", []) => Some(
                            scope
                                .root_as_u64(root)
                                .map(Value::U64)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("i8", []) => Some(
                            scope
                                .root_as_i8(root)
                                .map(Value::I8)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("i16", []) => Some(
                            scope
                                .root_as_i16(root)
                                .map(Value::I16)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("i32", []) => Some(
                            scope
                                .root_as_i32(root)
                                .map(Value::I32)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("i64", []) => Some(
                            scope
                                .root_as_i64(root)
                                .map(Value::I64)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("f32", []) => Some(
                            scope
                                .root_as_f32(root)
                                .map(Value::F32)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("f64", []) => Some(
                            scope
                                .root_as_f64(root)
                                .map(Value::F64)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("Char", []) => Some(
                            scope
                                .root_as_char(root)
                                .map(Value::Char)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("String", []) => Some(
                            scope
                                .root_as_string(root)
                                .map(Value::String)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("UUID", []) => Some(
                            scope
                                .root_as_uuid(root)
                                .map(Value::Uuid)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("Hash", []) => Some(
                            scope
                                .root_as_hash(root)
                                .map(Value::Hash)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        ("DateTime", []) => Some(
                            scope
                                .root_as_datetime(root)
                                .map(Value::DateTime)
                                .map_err(|_| mismatch(scope))?,
                        ),
                        _ => None,
                    };
                    if let Some(value) = scalar {
                        values.push(value);
                        continue;
                    }

                    match (name.as_ref(), args.as_slice()) {
                        ("List", [element_type]) if is_u8_type(element_type) => {
                            values.push(
                                scope
                                    .root_as_binary_list(root)
                                    .map(Value::Bytes)
                                    .map_err(|_| mismatch(scope))?,
                            );
                        }
                        ("List", [element_type]) => {
                            let items = scope.root_as_list(root).map_err(|_| mismatch(scope))?;
                            work.push(ExportWork::List(items.len()));
                            for (index, item) in items.into_iter().enumerate().rev() {
                                work.push(ExportWork::Convert {
                                    root: item,
                                    expected: element_type.clone(),
                                    path: path.child(format!("[{index}]")),
                                });
                            }
                        }
                        ("Dict", [element_type]) => {
                            let fields = scope.root_as_dict(root).map_err(|_| mismatch(scope))?;
                            let names = fields.keys().cloned().collect::<Vec<_>>();
                            work.push(ExportWork::Dict(names.clone()));
                            for name in names.into_iter().rev() {
                                work.push(ExportWork::Convert {
                                    root: fields[&name],
                                    expected: element_type.clone(),
                                    path: path.child(format!(".{name}")),
                                });
                            }
                        }
                        _ => {
                            let (runtime_tag, fields) =
                                scope.root_as_adt(root).map_err(|_| mismatch(scope))?;
                            let (_, variant, field_types) =
                                resolve_adt_variant(types, &name, &args, &runtime_tag, &path)?;
                            if fields.len() != field_types.len() {
                                return Err(conversion_error(
                                    &path,
                                    &expected,
                                    format!(
                                        "constructor {} with {} fields",
                                        runtime_tag,
                                        fields.len()
                                    ),
                                ));
                            }
                            let tag = Symbol::intern(local_name(&variant.name));
                            work.push(ExportWork::Adt {
                                tag: tag.clone(),
                                fields: fields.len(),
                            });
                            for (index, (field, field_type)) in
                                fields.into_iter().zip(field_types).enumerate().rev()
                            {
                                work.push(ExportWork::Convert {
                                    root: field,
                                    expected: field_type,
                                    path: path.child(format!(".{tag}[{index}]")),
                                });
                            }
                        }
                    }
                }
            },
            ExportWork::Tuple(len) => {
                let fields = take_value_tail(&mut values, len)?;
                values.push(Value::Tuple(fields));
            }
            ExportWork::List(len) => {
                let items = take_value_tail(&mut values, len)?;
                values.push(Value::List(items));
            }
            ExportWork::Dict(names) => {
                let fields = take_value_tail(&mut values, names.len())?;
                values.push(Value::Dict(names.into_iter().zip(fields).collect()));
            }
            ExportWork::Adt { tag, fields } => {
                let fields = take_value_tail(&mut values, fields)?;
                values.push(Value::Adt(tag, fields));
            }
        }
    }

    if values.len() != 1 {
        return Err(EngineError::Internal(format!(
            "heap-to-value conversion produced {} roots",
            values.len()
        )));
    }
    values
        .pop()
        .ok_or_else(|| EngineError::Internal("heap-to-value conversion produced no result".into()))
}

enum ImportWork {
    Convert {
        value: Value,
        expected: Type,
        path: ConversionPath,
    },
    Tuple(usize),
    List(usize),
    Dict(Vec<String>),
    Adt {
        tag: Symbol,
        fields: usize,
    },
}

fn value_into_root(
    scope: &mut RootScope<'_>,
    value: Value,
    expected: &Type,
    types: &TypeSystem,
    path: ConversionPath,
) -> Result<RootedPtr, EngineError> {
    let mut work = vec![ImportWork::Convert {
        value,
        expected: expected.clone(),
        path,
    }];
    let mut roots = Vec::new();

    while let Some(item) = work.pop() {
        match item {
            ImportWork::Convert {
                value,
                expected,
                path,
            } => {
                let got = value.value_type_name();
                match expected.as_ref() {
                    TypeKind::Var(_) => {
                        return Err(conversion_error(
                            &path,
                            &expected,
                            "unresolved type variable",
                        ));
                    }
                    TypeKind::Fun(_, _) => {
                        return Err(conversion_error(&path, &expected, got));
                    }
                    TypeKind::Tuple(item_types) => {
                        let Value::Tuple(items) = value else {
                            return Err(conversion_error(&path, &expected, got));
                        };
                        if items.len() != item_types.len() {
                            return Err(conversion_error(
                                &path,
                                &expected,
                                format!("tuple with {} items", items.len()),
                            ));
                        }
                        work.push(ImportWork::Tuple(items.len()));
                        for (index, (item, item_type)) in
                            items.into_iter().zip(item_types).enumerate().rev()
                        {
                            work.push(ImportWork::Convert {
                                value: item,
                                expected: item_type.clone(),
                                path: path.child(format!("[{index}]")),
                            });
                        }
                    }
                    TypeKind::Record(field_types) => {
                        let Value::Dict(mut fields) = value else {
                            return Err(conversion_error(&path, &expected, got));
                        };
                        if fields.len() != field_types.len() {
                            return Err(conversion_error(
                                &path,
                                &expected,
                                format!("record with {} fields", fields.len()),
                            ));
                        }
                        let definitions = field_types.clone();
                        let names = definitions
                            .iter()
                            .map(|(name, _)| name.to_string())
                            .collect::<Vec<_>>();
                        let mut children = Vec::with_capacity(names.len());
                        for (name, field_type) in definitions {
                            let field = fields.remove(name.as_ref()).ok_or_else(|| {
                                conversion_error(
                                    path.child(format!(".{name}")),
                                    &field_type,
                                    "missing field",
                                )
                            })?;
                            children.push((name.to_string(), field, field_type));
                        }
                        work.push(ImportWork::Dict(names));
                        for (name, field, field_type) in children.into_iter().rev() {
                            work.push(ImportWork::Convert {
                                value: field,
                                expected: field_type,
                                path: path.child(format!(".{name}")),
                            });
                        }
                    }
                    TypeKind::Con(_) | TypeKind::App(_, _) => {
                        let (head, args) = decompose_type_app(&expected);
                        let TypeKind::Con(con) = head.as_ref() else {
                            return Err(conversion_error(&path, &expected, got));
                        };
                        let name = con.name();
                        let root = match (name.as_ref(), args.as_slice(), value) {
                            ("Bool", [], Value::Bool(value)) => Some(scope.alloc_root_bool(value)?),
                            ("u8", [], Value::U8(value)) => Some(scope.alloc_root_u8(value)?),
                            ("u16", [], Value::U16(value)) => Some(scope.alloc_root_u16(value)?),
                            ("u32", [], Value::U32(value)) => Some(scope.alloc_root_u32(value)?),
                            ("u64", [], Value::U64(value)) => Some(scope.alloc_root_u64(value)?),
                            ("i8", [], Value::I8(value)) => Some(scope.alloc_root_i8(value)?),
                            ("i16", [], Value::I16(value)) => Some(scope.alloc_root_i16(value)?),
                            ("i32", [], Value::I32(value)) => Some(scope.alloc_root_i32(value)?),
                            ("i64", [], Value::I64(value)) => Some(scope.alloc_root_i64(value)?),
                            ("f32", [], Value::F32(value)) => Some(scope.alloc_root_f32(value)?),
                            ("f64", [], Value::F64(value)) => Some(scope.alloc_root_f64(value)?),
                            ("Char", [], Value::Char(value)) => Some(scope.alloc_root_char(value)?),
                            ("String", [], Value::String(value)) => {
                                Some(scope.alloc_root_string(value)?)
                            }
                            ("UUID", [], Value::Uuid(value)) => Some(scope.alloc_root_uuid(value)?),
                            ("Hash", [], Value::Hash(value)) => Some(scope.alloc_root_hash(value)?),
                            ("DateTime", [], Value::DateTime(value)) => {
                                Some(scope.alloc_root_datetime(value)?)
                            }
                            ("List", [element_type], Value::Bytes(bytes))
                                if is_u8_type(element_type) =>
                            {
                                Some(scope.alloc_root_binary_list(bytes)?)
                            }
                            ("List", [element_type], Value::List(items))
                                if !is_u8_type(element_type) =>
                            {
                                work.push(ImportWork::List(items.len()));
                                for (index, item) in items.into_iter().enumerate().rev() {
                                    work.push(ImportWork::Convert {
                                        value: item,
                                        expected: element_type.clone(),
                                        path: path.child(format!("[{index}]")),
                                    });
                                }
                                None
                            }
                            ("Dict", [element_type], Value::Dict(fields)) => {
                                let names = fields.keys().cloned().collect::<Vec<_>>();
                                let children = fields.into_iter().collect::<Vec<_>>();
                                work.push(ImportWork::Dict(names));
                                for (name, value) in children.into_iter().rev() {
                                    work.push(ImportWork::Convert {
                                        value,
                                        expected: element_type.clone(),
                                        path: path.child(format!(".{name}")),
                                    });
                                }
                                None
                            }
                            (_, _, Value::Adt(tag, fields)) => {
                                let (_, variant, field_types) =
                                    resolve_adt_variant(types, &name, &args, &tag, &path)?;
                                if fields.len() != field_types.len() {
                                    return Err(conversion_error(
                                        &path,
                                        &expected,
                                        format!("constructor {} with {} fields", tag, fields.len()),
                                    ));
                                }
                                let tag = Symbol::intern(local_name(&variant.name));
                                work.push(ImportWork::Adt {
                                    tag: tag.clone(),
                                    fields: fields.len(),
                                });
                                for (index, (field, field_type)) in
                                    fields.into_iter().zip(field_types).enumerate().rev()
                                {
                                    work.push(ImportWork::Convert {
                                        value: field,
                                        expected: field_type,
                                        path: path.child(format!(".{tag}[{index}]")),
                                    });
                                }
                                None
                            }
                            _ => {
                                return Err(conversion_error(&path, &expected, got));
                            }
                        };
                        if let Some(root) = root {
                            roots.push(root);
                        }
                    }
                }
            }
            ImportWork::Tuple(len) => {
                let fields = take_root_tail(&mut roots, len)?;
                roots.push(scope.alloc_root_tuple(fields)?);
            }
            ImportWork::List(len) => {
                let items = take_root_tail(&mut roots, len)?;
                roots.push(scope.alloc_root_list(items)?);
            }
            ImportWork::Dict(names) => {
                let fields = take_root_tail(&mut roots, names.len())?;
                roots.push(scope.alloc_root_dict(names.into_iter().zip(fields).collect())?);
            }
            ImportWork::Adt { tag, fields } => {
                let fields = take_root_tail(&mut roots, fields)?;
                roots.push(scope.alloc_root_adt(tag, fields)?);
            }
        }
    }

    if roots.len() != 1 {
        return Err(EngineError::Internal(format!(
            "value-to-heap conversion produced {} roots",
            roots.len()
        )));
    }
    roots
        .pop()
        .ok_or_else(|| EngineError::Internal("value-to-heap conversion produced no result".into()))
}

fn take_value_tail(values: &mut Vec<Value>, len: usize) -> Result<Vec<Value>, EngineError> {
    let start = values
        .len()
        .checked_sub(len)
        .ok_or_else(|| EngineError::Internal("heap-to-value conversion stack underflow".into()))?;
    Ok(values.split_off(start))
}

fn take_root_tail(roots: &mut Vec<RootedPtr>, len: usize) -> Result<Vec<RootedPtr>, EngineError> {
    let start = roots
        .len()
        .checked_sub(len)
        .ok_or_else(|| EngineError::Internal("value-to-heap conversion stack underflow".into()))?;
    Ok(roots.split_off(start))
}

fn resolve_adt_variant<'a>(
    types: &'a TypeSystem,
    adt_name: &Symbol,
    type_args: &[Type],
    tag: &Symbol,
    path: &ConversionPath,
) -> Result<
    (
        &'a AdtDecl,
        &'a rex_typesystem::types::AdtVariant,
        Vec<Type>,
    ),
    EngineError,
> {
    let direct_adt = types
        .adts
        .get(adt_name)
        .ok_or_else(|| conversion_error(path, adt_name, format!("unknown ADT `{adt_name}`")))?;
    let adt = direct_adt;
    if adt.params.len() != type_args.len() {
        return Err(conversion_error(
            path,
            adt_name,
            format!("{} type arguments", type_args.len()),
        ));
    }
    let variant = adt
        .variants
        .iter()
        .find(|variant| {
            variant.name == *tag
                || (!tag.as_ref().contains('.') && local_name(&variant.name) == tag.as_ref())
        })
        .ok_or_else(|| {
            conversion_error(
                path,
                adt_name,
                format!(
                    "constructor `{tag}` from another ADT (expected one of: {})",
                    adt.variants
                        .iter()
                        .map(|variant| variant.name.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })?;
    let substitutions = adt
        .params
        .iter()
        .zip(type_args)
        .map(|(parameter, value)| (parameter.var.id, value.clone()))
        .collect::<BTreeMap<_, _>>();
    let field_types = variant
        .args
        .iter()
        .map(|field| instantiate_type(&field.typ(), &substitutions))
        .collect();
    Ok((adt, variant, field_types))
}

fn instantiate_type(value: &Type, substitutions: &BTreeMap<usize, Type>) -> Type {
    match value.as_ref() {
        TypeKind::Var(variable) => substitutions
            .get(&variable.id)
            .cloned()
            .unwrap_or_else(|| value.clone()),
        TypeKind::Con(_) => value.clone(),
        TypeKind::App(function, argument) => Type::app(
            instantiate_type(function, substitutions),
            instantiate_type(argument, substitutions),
        ),
        TypeKind::Fun(argument, result) => Type::fun(
            instantiate_type(argument, substitutions),
            instantiate_type(result, substitutions),
        ),
        TypeKind::Tuple(items) => Type::tuple(
            items
                .iter()
                .map(|item| instantiate_type(item, substitutions))
                .collect::<Vec<_>>(),
        ),
        TypeKind::Record(fields) => Type::record(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), instantiate_type(field, substitutions)))
                .collect::<Vec<_>>(),
        ),
    }
}

fn decompose_type_app(value: &Type) -> (Type, Vec<Type>) {
    let mut arguments = Vec::new();
    let mut head = value.clone();
    while let TypeKind::App(function, argument) = head.as_ref() {
        arguments.push(argument.clone());
        head = function.clone();
    }
    arguments.reverse();
    (head, arguments)
}

fn is_u8_type(value: &Type) -> bool {
    matches!(value.as_ref(), TypeKind::Con(con) if con.name().as_ref() == "u8")
}

fn local_name(value: &Symbol) -> &str {
    value.as_ref().rsplit('.').next().unwrap_or(value.as_ref())
}

fn root_type_name(scope: &RootScope<'_>, root: RootedPtr) -> String {
    scope
        .type_name(root)
        .unwrap_or("invalid heap value")
        .to_string()
}

fn conversion_error(
    path: impl ToString,
    expected: impl ToString,
    got: impl ToString,
) -> EngineError {
    EngineError::ValueConversion {
        path: path.to_string(),
        expected: expected.to_string(),
        got: got.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{memory::heap::Heap, prelude::standard_type_system};
    use rex_typesystem::{
        types::{AdtArgument, BuiltinTypeId},
        typesystem::TypeVarSupply,
    };
    use static_assertions::assert_impl_all;

    assert_impl_all!(Value: Send, Sync);

    fn builtin(id: BuiltinTypeId) -> Type {
        Type::builtin(id)
    }

    #[test]
    fn list_u8_is_canonicalized_from_every_physical_layout() {
        let types = standard_type_system().unwrap();
        let expected = Type::list(builtin(BuiltinTypeId::U8));
        let mut heap = Heap::new();

        let values = heap
            .root_scope(|scope| {
                let empty = scope.alloc_root_empty()?;

                let ordinary_items = [1, 2, 3]
                    .into_iter()
                    .map(|value| scope.alloc_root_u8(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let ordinary = scope.alloc_root_list(ordinary_items)?;

                let data_items = [2, 3, 4]
                    .into_iter()
                    .map(|value| scope.alloc_root_u8(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let data = scope.alloc_root_data(data_items)?;
                let data_slice = scope.alloc_root_list_slice(0, 2, data)?;
                let data_head = scope.alloc_root_u8(1)?;
                let data_hybrid = scope.alloc_root_cons(data_head, data_slice)?;

                let binary_data = scope.alloc_root_binary_data(vec![2, 3, 4])?;
                let binary_slice = scope.alloc_root_list_slice(0, 2, binary_data)?;
                let binary_head = scope.alloc_root_u8(1)?;
                let binary_hybrid = scope.alloc_root_cons(binary_head, binary_slice)?;

                [empty, ordinary, data_hybrid, binary_hybrid]
                    .into_iter()
                    .map(|root| scope.export_value(root, &expected, &types))
                    .collect::<Result<Vec<_>, EngineError>>()
            })
            .unwrap();

        assert_eq!(
            values,
            vec![
                Value::Bytes(vec![]),
                Value::Bytes(vec![1, 2, 3]),
                Value::Bytes(vec![1, 2, 3]),
                Value::Bytes(vec![1, 2, 3]),
            ]
        );
    }

    #[test]
    fn byte_list_host_representation_is_strictly_canonical() {
        let types = standard_type_system().unwrap();
        let expected = Type::list(builtin(BuiltinTypeId::U8));
        let mut heap = Heap::new();

        heap.root_scope(|scope| {
            let root = scope.alloc_value(Value::Bytes(vec![1, 2, 3]), &expected, &types)?;
            assert_eq!(
                scope.export_value(root, &expected, &types)?,
                Value::Bytes(vec![1, 2, 3])
            );

            let error = scope
                .alloc_value(
                    Value::List(vec![Value::U8(1), Value::U8(2)]),
                    &expected,
                    &types,
                )
                .unwrap_err();
            assert!(matches!(
                error,
                EngineError::ValueConversion { ref path, .. } if path == "$"
            ));
            Ok::<(), EngineError>(())
        })
        .unwrap();
    }

    #[test]
    fn owned_composites_round_trip_and_errors_identify_nested_paths() {
        let types = standard_type_system().unwrap();
        let expected = Type::tuple([
            Type::list(builtin(BuiltinTypeId::I32)),
            Type::record([
                (Symbol::intern("enabled"), builtin(BuiltinTypeId::Bool)),
                (Symbol::intern("name"), builtin(BuiltinTypeId::String)),
            ]),
            Type::option(builtin(BuiltinTypeId::I32)),
        ]);
        let value = Value::Tuple(vec![
            Value::List(vec![Value::I32(1), Value::I32(2)]),
            Value::Dict(
                [
                    ("enabled".to_owned(), Value::Bool(true)),
                    ("name".to_owned(), Value::String("sample".into())),
                ]
                .into_iter()
                .collect(),
            ),
            Value::Adt(Symbol::intern("Some"), vec![Value::I32(3)]),
        ]);
        let mut heap = Heap::new();

        heap.root_scope(|scope| {
            let root = scope.alloc_value(value.clone(), &expected, &types)?;
            assert_eq!(scope.export_value(root, &expected, &types)?, value);

            let wrong = Type::tuple([builtin(BuiltinTypeId::I32), builtin(BuiltinTypeId::I32)]);
            let error = scope
                .alloc_value(
                    Value::Tuple(vec![Value::I32(1), Value::Bool(false)]),
                    &wrong,
                    &types,
                )
                .unwrap_err();
            assert!(matches!(
                error,
                EngineError::ValueConversion {
                    ref path,
                    ref expected,
                    ref got,
                } if path == "$[1]" && expected == "i32" && got == "Bool"
            ));
            Ok::<(), EngineError>(())
        })
        .unwrap();
    }

    #[test]
    fn function_values_and_unknown_constructors_are_rejected() {
        let types = standard_type_system().unwrap();
        let function = Type::fun(builtin(BuiltinTypeId::I32), builtin(BuiltinTypeId::I32));
        let option = Type::option(builtin(BuiltinTypeId::I32));
        let mut heap = Heap::new();

        heap.root_scope(|scope| {
            let scalar = scope.alloc_root_i32(1)?;
            assert!(matches!(
                scope.export_value(scalar, &function, &types),
                Err(EngineError::ValueConversion { .. })
            ));
            assert!(matches!(
                scope.alloc_value(Value::I32(1), &function, &types),
                Err(EngineError::ValueConversion { .. })
            ));
            assert!(matches!(
                scope.alloc_value(Value::Adt(Symbol::intern("Bogus"), vec![]), &option, &types,),
                Err(EngineError::ValueConversion { .. })
            ));
            assert!(matches!(
                scope.alloc_value(
                    Value::Adt(Symbol::intern("another.module.Some"), vec![Value::I32(1)]),
                    &option,
                    &types,
                ),
                Err(EngineError::ValueConversion { .. })
            ));
            Ok::<(), EngineError>(())
        })
        .unwrap();
    }

    #[test]
    fn deeply_nested_adts_convert_without_using_the_rust_call_stack() {
        const DEPTH: usize = 10_000;

        let name = Symbol::intern("Chain");
        let expected = Type::con(name.clone(), 0);
        let mut supply = TypeVarSupply::new();
        let mut declaration = AdtDecl::new(&name, &[], &mut supply);
        declaration.add_variant(Symbol::intern("End"), vec![], None);
        declaration.add_variant(
            Symbol::intern("Next"),
            vec![AdtArgument::positional(expected.clone())],
            None,
        );
        let mut types = standard_type_system().unwrap();
        types.register_adt(&declaration).unwrap();

        let mut input = Value::Adt(Symbol::intern("End"), vec![]);
        for _ in 0..DEPTH {
            input = Value::Adt(Symbol::intern("Next"), vec![input]);
        }

        let mut heap = Heap::new();
        let mut output = heap
            .root_scope(|scope| {
                let root = scope.alloc_value(input, &expected, &types)?;
                scope.export_value(root, &expected, &types)
            })
            .unwrap();

        for _ in 0..DEPTH {
            let Value::Adt(tag, mut fields) = output else {
                panic!("expected Next constructor");
            };
            assert_eq!(tag.as_ref(), "Next");
            assert_eq!(fields.len(), 1);
            output = fields.pop().unwrap();
        }
        assert_eq!(output, Value::Adt(Symbol::intern("End"), vec![]));
    }
}
