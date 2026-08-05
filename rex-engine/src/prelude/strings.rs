use rex_typesystem::types::{BuiltinTypeId, Scheme, Type};

use crate::{EngineError, builder::core::Builder};

/// Register the documented string built-ins with their runtime implementations.
pub(super) fn inject_string_builtins<State>(engine: &mut Builder<State>) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let char_ty = Type::builtin(BuiltinTypeId::Char);
    let string_ty = Type::builtin(BuiltinTypeId::String);
    let u8_ty = Type::builtin(BuiltinTypeId::U8);
    let u64_ty = Type::builtin(BuiltinTypeId::U64);
    let list_char_ty = Type::list(char_ty.clone());
    let list_string_ty = Type::list(string_ty.clone());
    let list_u8_ty = Type::list(u8_ty);
    let option_char_ty = Type::option(char_ty);
    let option_string_ty = Type::option(string_ty.clone());
    let option_u64_ty = Type::option(u64_ty.clone());

    engine.export_native(
        "string_get",
        monomorphic(&[u64_ty.clone(), string_ty.clone()], option_char_ty),
        2,
        |scope, _, args| {
            let index = scope.root_as_u64(args[0])?;
            let input = scope.root_as_string(args[1])?;
            let value = match string_get(index, &input) {
                Some(value) => Some(scope.alloc_root_char(value)?),
                None => None,
            };
            super::option_from_root(scope, value)
        },
    )?;
    engine.export_native(
        "string_slice",
        monomorphic(
            &[u64_ty.clone(), u64_ty.clone(), string_ty.clone()],
            option_string_ty.clone(),
        ),
        3,
        |scope, _, args| {
            let start = scope.root_as_u64(args[0])?;
            let end = scope.root_as_u64(args[1])?;
            let input = scope.root_as_string(args[2])?;
            let value = match string_slice(start, end, &input) {
                Some(value) => Some(scope.alloc_root_string(value)?),
                None => None,
            };
            super::option_from_root(scope, value)
        },
    )?;
    engine.export_native(
        "string_contains",
        monomorphic(&[string_ty.clone(), string_ty.clone()], bool_ty.clone()),
        2,
        |scope, _, args| {
            let needle = scope.root_as_string(args[0])?;
            let haystack = scope.root_as_string(args[1])?;
            scope.alloc_root_bool(string_contains(&needle, &haystack))
        },
    )?;
    engine.export_native(
        "string_starts_with",
        monomorphic(&[string_ty.clone(), string_ty.clone()], bool_ty.clone()),
        2,
        |scope, _, args| {
            let prefix = scope.root_as_string(args[0])?;
            let input = scope.root_as_string(args[1])?;
            scope.alloc_root_bool(string_starts_with(&prefix, &input))
        },
    )?;
    engine.export_native(
        "string_ends_with",
        monomorphic(&[string_ty.clone(), string_ty.clone()], bool_ty),
        2,
        |scope, _, args| {
            let suffix = scope.root_as_string(args[0])?;
            let input = scope.root_as_string(args[1])?;
            scope.alloc_root_bool(string_ends_with(&suffix, &input))
        },
    )?;
    engine.export_native(
        "string_find",
        monomorphic(&[string_ty.clone(), string_ty.clone()], option_u64_ty),
        2,
        |scope, _, args| {
            let needle = scope.root_as_string(args[0])?;
            let haystack = scope.root_as_string(args[1])?;
            let value = match string_find(&needle, &haystack) {
                Some(value) => Some(scope.alloc_root_u64(value)?),
                None => None,
            };
            super::option_from_root(scope, value)
        },
    )?;
    engine.export_native(
        "string_split",
        monomorphic(
            &[string_ty.clone(), string_ty.clone()],
            list_string_ty.clone(),
        ),
        2,
        |scope, _, args| {
            let separator = scope.root_as_string(args[0])?;
            let input = scope.root_as_string(args[1])?;
            let parts = string_split(&separator, &input);
            let mut roots = Vec::with_capacity(parts.len());
            for part in parts {
                roots.push(scope.alloc_root_string(part)?);
            }
            scope.alloc_root_list(roots)
        },
    )?;
    engine.export_native(
        "string_join",
        monomorphic(&[string_ty.clone(), list_string_ty], string_ty.clone()),
        2,
        |scope, _, args| {
            let separator = scope.root_as_string(args[0])?;
            let items = scope.list_items(args[1])?;
            let mut parts = Vec::with_capacity(items.len());
            for index in 0..items.len() {
                let item = items.get(scope, index)?;
                parts.push(scope.root_as_string(item)?);
            }
            scope.alloc_root_string(string_join(&separator, &parts))
        },
    )?;
    engine.export_native(
        "string_replace",
        monomorphic(
            &[string_ty.clone(), string_ty.clone(), string_ty.clone()],
            string_ty.clone(),
        ),
        3,
        |scope, _, args| {
            let needle = scope.root_as_string(args[0])?;
            let replacement = scope.root_as_string(args[1])?;
            let input = scope.root_as_string(args[2])?;
            scope.alloc_root_string(string_replace(&needle, &replacement, &input))
        },
    )?;

    macro_rules! export_unary_string {
        ($name:literal, $function:ident) => {
            engine.export_native(
                $name,
                monomorphic(std::slice::from_ref(&string_ty), string_ty.clone()),
                1,
                |scope, _, args| {
                    let input = scope.root_as_string(args[0])?;
                    scope.alloc_root_string($function(&input))
                },
            )?;
        };
    }
    export_unary_string!("string_trim", string_trim);
    export_unary_string!("string_trim_start", string_trim_start);
    export_unary_string!("string_trim_end", string_trim_end);
    export_unary_string!("string_to_lower", string_to_lower);
    export_unary_string!("string_to_upper", string_to_upper);

    engine.export_native(
        "string_to_chars",
        monomorphic(std::slice::from_ref(&string_ty), list_char_ty.clone()),
        1,
        |scope, _, args| {
            let input = scope.root_as_string(args[0])?;
            let chars = string_to_chars(&input);
            let mut roots = Vec::with_capacity(chars.len());
            for value in chars {
                roots.push(scope.alloc_root_char(value)?);
            }
            scope.alloc_root_list(roots)
        },
    )?;
    engine.export_native(
        "chars_to_string",
        monomorphic(&[list_char_ty], string_ty.clone()),
        1,
        |scope, _, args| {
            let items = scope.list_items(args[0])?;
            let mut chars = Vec::with_capacity(items.len());
            for index in 0..items.len() {
                let item = items.get(scope, index)?;
                chars.push(scope.root_as_char(item)?);
            }
            scope.alloc_root_string(chars_to_string(chars))
        },
    )?;
    engine.export_native(
        "string_to_utf8",
        monomorphic(std::slice::from_ref(&string_ty), list_u8_ty.clone()),
        1,
        |scope, _, args| {
            let input = scope.root_as_string(args[0])?;
            scope.alloc_root_binary_list(string_to_utf8(input))
        },
    )?;
    engine.export_native(
        "utf8_to_string",
        monomorphic(&[list_u8_ty], option_string_ty),
        1,
        |scope, _, args| {
            let items = scope.list_items(args[0])?;
            let mut bytes = Vec::with_capacity(items.len());
            for index in 0..items.len() {
                let item = items.get(scope, index)?;
                bytes.push(scope.root_as_u8(item)?);
            }
            let value = match utf8_to_string(bytes) {
                Some(value) => Some(scope.alloc_root_string(value)?),
                None => None,
            };
            super::option_from_root(scope, value)
        },
    )?;
    Ok(())
}

/// Build a monomorphic curried function scheme from `args` to `result`.
fn monomorphic(args: &[Type], result: Type) -> Scheme {
    let typ = args
        .iter()
        .rev()
        .fold(result, |out, arg| Type::fun(arg.clone(), out));
    Scheme::new(vec![], vec![], typ)
}

/// Return the character at zero-based Unicode scalar `index` in `input`.
fn string_get(index: u64, input: &str) -> Option<char> {
    let index = usize::try_from(index).ok()?;
    input.chars().nth(index)
}

/// Return the half-open Unicode scalar range `start..end` from `input`.
fn string_slice(start: u64, end: u64, input: &str) -> Option<String> {
    if end < start {
        return None;
    }
    let start = scalar_boundary(input, start)?;
    let end = scalar_boundary(input, end)?;
    input.get(start..end).map(str::to_owned)
}

/// Return whether `haystack` contains the substring `needle`.
fn string_contains(needle: &str, haystack: &str) -> bool {
    haystack.contains(needle)
}

/// Return whether `input` begins with the substring `prefix`.
fn string_starts_with(prefix: &str, input: &str) -> bool {
    input.starts_with(prefix)
}

/// Return whether `input` ends with the substring `suffix`.
fn string_ends_with(suffix: &str, input: &str) -> bool {
    input.ends_with(suffix)
}

/// Return the first Unicode scalar index of `needle` within `haystack`.
fn string_find(needle: &str, haystack: &str) -> Option<u64> {
    let byte_index = haystack.find(needle)?;
    u64::try_from(haystack[..byte_index].chars().count()).ok()
}

/// Split `input` at each non-overlapping occurrence of `separator`.
fn string_split(separator: &str, input: &str) -> Vec<String> {
    input.split(separator).map(str::to_owned).collect()
}

/// Join `parts`, inserting `separator` between adjacent strings.
fn string_join(separator: &str, parts: &[String]) -> String {
    parts.join(separator)
}

/// Replace each non-overlapping `needle` in `input` with `replacement`.
fn string_replace(needle: &str, replacement: &str, input: &str) -> String {
    input.replace(needle, replacement)
}

/// Remove Unicode whitespace from both ends of `input`.
fn string_trim(input: &str) -> String {
    input.trim().to_owned()
}

/// Remove Unicode whitespace from the start of `input`.
fn string_trim_start(input: &str) -> String {
    input.trim_start().to_owned()
}

/// Remove Unicode whitespace from the end of `input`.
fn string_trim_end(input: &str) -> String {
    input.trim_end().to_owned()
}

/// Convert `input` with the Unicode lowercase mapping.
fn string_to_lower(input: &str) -> String {
    input.to_lowercase()
}

/// Convert `input` with the Unicode uppercase mapping.
fn string_to_upper(input: &str) -> String {
    input.to_uppercase()
}

/// Collect the Unicode scalar values in `input` into a character list.
fn string_to_chars(input: &str) -> Vec<char> {
    input.chars().collect()
}

/// Concatenate `chars` in order into a string.
fn chars_to_string(chars: Vec<char>) -> String {
    chars.into_iter().collect()
}

/// Encode `input` as its UTF-8 byte sequence.
fn string_to_utf8(input: String) -> Vec<u8> {
    input.into_bytes()
}

/// Decode `bytes` as UTF-8, returning `None` when the sequence is invalid.
fn utf8_to_string(bytes: Vec<u8>) -> Option<String> {
    String::from_utf8(bytes).ok()
}

/// Translate Unicode scalar `index` in `input` to a UTF-8 byte boundary.
fn scalar_boundary(input: &str, index: u64) -> Option<usize> {
    let index = usize::try_from(index).ok()?;
    input
        .char_indices()
        .map(|(byte_index, _)| byte_index)
        .chain(std::iter::once(input.len()))
        .nth(index)
}
