#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use rex::{
    ast::{CompilationUnit, Decl, Symbol},
    engine::{prelude_typeclasses_program, standard_type_system},
    typesystem::{Instance, Predicate, Scheme, Type, TypeKind, TypeSystem},
};

const OUTPUT_PATH: &str = "docs/src/BUILTINS.md";
const DESCRIPTIONS_PATH: &str = "docs/src/prelude_descriptions.txt";

struct FunctionSection {
    title: &'static str,
    introduction: &'static str,
    functions: &'static [&'static str],
}

const FUNCTION_SECTIONS: &[FunctionSection] = &[
    FunctionSection {
        title: "Boolean Operations",
        introduction: "Boolean operators combine two `Bool` values.",
        functions: &["&&", "||"],
    },
    FunctionSection {
        title: "Comparison Operations",
        introduction: "Equality is available for all listed equality-comparable types. Ordering operations are available for numbers, characters, and strings.",
        functions: &["==", "!=", "<", "<=", ">", ">=", "cmp"],
    },
    FunctionSection {
        title: "Ordering Values",
        introduction: "`cmp` returns one of these `Ordering` values.",
        functions: &["Less", "Equal", "Greater"],
    },
    FunctionSection {
        title: "Arithmetic and Aggregation",
        introduction: "Arithmetic operators work on the numeric types listed for each operation. Some operations are also overloaded for lists or strings where noted.",
        functions: &[
            "zero", "one", "+", "-", "*", "/", "%", "negate", "sum", "mean", "min", "max",
        ],
    },
    FunctionSection {
        title: "General Value Functions",
        introduction: "Functions for constructing defaults, parsing strings, and rendering values.",
        functions: &["default", "parse", "show"],
    },
    FunctionSection {
        title: "Collection and Container Functions",
        introduction: "Generic operations shared by lists, options, results, dictionaries, or strings. Check the availability list on each function.",
        functions: &[
            "length",
            "map",
            "filter",
            "filter_map",
            "foldl",
            "foldr",
            "fold",
            "pure",
            "ap",
            "bind",
            "or_else",
        ],
    },
    FunctionSection {
        title: "List Functions",
        introduction: "Construct, index, slice, and combine `List` values.",
        functions: &[
            "Empty", "Cons", "get", "take", "skip", "first", "last", "slice", "zip", "unzip",
        ],
    },
    FunctionSection {
        title: "Dict Functions",
        introduction: "Dictionary-specific operations. Keys are strings, dictionaries are immutable, and operations that modify a dictionary return a new value.",
        functions: &[
            "dict_empty",
            "dict_singleton",
            "dict_get",
            "dict_has",
            "dict_insert",
            "dict_remove",
            "dict_update",
            "dict_is_empty",
            "dict_keys",
            "dict_values",
            "dict_entries",
            "dict_from_entries",
            "dict_map",
            "dict_filter",
        ],
    },
    FunctionSection {
        title: "String Functions",
        introduction: "String indexing and positions count Unicode scalar values, not UTF-8 bytes. Functions with selector or modifier arguments place those arguments before the input string.",
        functions: &[
            "string_get",
            "string_slice",
            "string_contains",
            "string_starts_with",
            "string_ends_with",
            "string_find",
            "string_split",
            "string_join",
            "string_replace",
            "string_trim",
            "string_trim_start",
            "string_trim_end",
            "string_to_lower",
            "string_to_upper",
            "string_to_chars",
            "chars_to_string",
            "string_to_utf8",
            "utf8_to_string",
        ],
    },
    FunctionSection {
        title: "Option and Result Functions",
        introduction: "Construct, inspect, and extract optional values and success-or-error results.",
        functions: &[
            "None", "Some", "is_none", "is_some", "Err", "Ok", "is_err", "is_ok", "unwrap",
        ],
    },
];

#[derive(Clone, Debug)]
struct TypeDoc {
    name: String,
    arity: usize,
    constructors: Vec<String>,
}

#[derive(Clone, Debug)]
struct FunctionDoc {
    name: String,
    signatures: Vec<String>,
    class: Option<String>,
    implemented_on: Vec<String>,
}

#[derive(Clone, Debug)]
struct DocEntry {
    call: Option<String>,
    description: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let descriptions = load_descriptions(Path::new(DESCRIPTIONS_PATH))?;
    let ts = standard_type_system().map_err(|e| format!("{e}"))?;
    let program = prelude_typeclasses_program().map_err(|e| format!("{e}"))?;

    let mut type_arity = BTreeMap::<String, usize>::new();
    collect_all_type_constructors(&ts, &mut type_arity);

    let primitive_type_names = collect_primitive_type_names(&ts);
    let methods_by_class = collect_methods_by_class(program)?;
    let types = build_types(&ts, &type_arity);
    let functions = build_functions(&ts, &methods_by_class, &primitive_type_names);

    validate_function_sections(&functions)?;

    let required_keys = required_description_keys(&types, &functions);
    let missing_keys: Vec<String> = required_keys
        .iter()
        .filter(|key| !descriptions.contains_key(*key))
        .cloned()
        .collect();
    if !missing_keys.is_empty() {
        return Err(format!(
            "missing descriptions in {}:\n{}",
            DESCRIPTIONS_PATH,
            missing_keys
                .into_iter()
                .map(|k| format!("  - {k}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    let markdown = render_markdown(&types, &functions, &descriptions)?;
    fs::write(OUTPUT_PATH, markdown).map_err(|e| format!("failed to write {OUTPUT_PATH}: {e}"))?;
    println!("wrote {OUTPUT_PATH}");
    Ok(())
}

fn load_descriptions(path: &Path) -> Result<HashMap<String, DocEntry>, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut descriptions = HashMap::new();
    for (line_no, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').map(str::trim).collect::<Vec<_>>();
        let (key, call, description) = match fields.as_slice() {
            [key, description] if !key.starts_with("fn:") => (*key, None, *description),
            [key, call, description] if key.starts_with("fn:") => {
                (*key, Some((*call).to_string()), *description)
            }
            _ => {
                return Err(format!(
                    "{}:{}: expected `key<TAB>description` for types or `fn:name<TAB>call<TAB>description` for functions",
                    path.display(),
                    line_no + 1
                ));
            }
        };
        let key = key.to_string();
        let description = description.to_string();
        if key.is_empty() || description.is_empty() {
            return Err(format!(
                "{}:{}: key and description must be non-empty",
                path.display(),
                line_no + 1
            ));
        }
        if call.as_ref().is_some_and(String::is_empty) {
            return Err(format!(
                "{}:{}: function call form must be non-empty",
                path.display(),
                line_no + 1
            ));
        }
        let entry = DocEntry { call, description };
        if descriptions.insert(key.clone(), entry).is_some() {
            return Err(format!(
                "{}:{}: duplicate key `{}`",
                path.display(),
                line_no + 1,
                key
            ));
        }
    }
    Ok(descriptions)
}

fn collect_primitive_type_names(ts: &TypeSystem) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (name, schemes) in ts.env.values.iter() {
        if schemes.len() != 1 {
            continue;
        }
        let scheme = &schemes[0];
        if !scheme.vars.is_empty() || !scheme.preds.is_empty() {
            continue;
        }
        if let TypeKind::Con(c) = scheme.typ.as_ref()
            && c.arity() == 0
            && c.name_str() == name.as_ref()
        {
            out.insert(name.to_string());
        }
    }
    out
}

fn collect_methods_by_class(
    compilation_unit: &CompilationUnit,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut out = BTreeMap::<String, Vec<String>>::new();
    for decl in &compilation_unit.decls {
        if let Decl::Class(class_decl) = decl {
            let class_name = class_decl.name.to_string();
            let methods = class_decl
                .methods
                .iter()
                .map(|m| m.name.to_string())
                .collect::<Vec<_>>();
            out.insert(class_name, methods);
        }
    }
    if out.is_empty() {
        return Err("no classes found in prelude_typeclasses program".into());
    }
    Ok(out)
}

fn collect_type_ctors_from_type(typ: &Type, out: &mut BTreeMap<String, usize>) {
    match typ.as_ref() {
        TypeKind::Var(_) => {}
        TypeKind::Con(c) => {
            out.entry(c.name_str().to_string())
                .and_modify(|arity| *arity = (*arity).max(c.arity()))
                .or_insert(c.arity());
        }
        TypeKind::App(l, r) | TypeKind::Fun(l, r) => {
            collect_type_ctors_from_type(l, out);
            collect_type_ctors_from_type(r, out);
        }
        TypeKind::Tuple(types) => {
            for t in types {
                collect_type_ctors_from_type(t, out);
            }
        }
        TypeKind::Record(fields) => {
            for (_, t) in fields {
                collect_type_ctors_from_type(t, out);
            }
        }
    }
}

fn collect_type_ctors_from_scheme(scheme: &Scheme, out: &mut BTreeMap<String, usize>) {
    for pred in &scheme.preds {
        collect_type_ctors_from_type(&pred.typ, out);
    }
    collect_type_ctors_from_type(&scheme.typ, out);
}

fn collect_all_type_constructors(ts: &TypeSystem, out: &mut BTreeMap<String, usize>) {
    for (_, schemes) in ts.env.values.iter() {
        for scheme in schemes {
            collect_type_ctors_from_scheme(scheme, out);
        }
    }
    for class_info in ts.class_info.values() {
        for scheme in class_info.methods.values() {
            collect_type_ctors_from_scheme(scheme, out);
        }
    }
    for instances in ts.classes.instances.values() {
        for inst in instances {
            for pred in &inst.context {
                collect_type_ctors_from_type(&pred.typ, out);
            }
            collect_type_ctors_from_type(&inst.head.typ, out);
        }
    }
    for (name, adt) in &ts.adts {
        out.entry(name.to_string())
            .and_modify(|arity| *arity = (*arity).max(adt.params.len()))
            .or_insert(adt.params.len());
        for variant in &adt.variants {
            for arg in &variant.args {
                collect_type_ctors_from_type(arg, out);
            }
        }
    }
}

fn format_type_head(name: &str, arity: usize) -> String {
    if arity == 0 {
        return name.to_string();
    }
    let vars = (0..arity)
        .map(|idx| ((b'a' + idx as u8) as char).to_string())
        .collect::<Vec<_>>()
        .join(" ");
    format!("{name} {vars}")
}

fn build_types(ts: &TypeSystem, type_arity: &BTreeMap<String, usize>) -> Vec<TypeDoc> {
    let mut constructors_by_type = HashMap::<String, Vec<String>>::new();
    for (type_name, adt) in &ts.adts {
        constructors_by_type.insert(
            type_name.to_string(),
            adt.variants.iter().map(|v| v.name.to_string()).collect(),
        );
    }

    let mut out = type_arity
        .iter()
        .map(|(name, arity)| TypeDoc {
            name: name.clone(),
            arity: *arity,
            constructors: constructors_by_type.remove(name).unwrap_or_default(),
        })
        .collect::<Vec<_>>();

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn format_predicate(pred: &Predicate) -> String {
    format!("{} {}", pred.class, pred.typ)
}

fn format_scheme(scheme: &Scheme) -> String {
    if scheme.preds.is_empty() {
        scheme.typ.to_string()
    } else {
        let preds = scheme
            .preds
            .iter()
            .map(format_predicate)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{preds} => {}", scheme.typ)
    }
}

fn format_instance_target(class_name: &str, inst: &Instance) -> String {
    if class_name == "Indexable"
        && let TypeKind::Tuple(types) = inst.head.typ.as_ref()
        && let Some(container) = types.first()
    {
        return container.to_string();
    }
    inst.head.typ.to_string()
}

fn build_functions(
    ts: &TypeSystem,
    methods_by_class: &BTreeMap<String, Vec<String>>,
    primitive_type_names: &BTreeSet<String>,
) -> Vec<FunctionDoc> {
    let class_for_method = methods_by_class
        .iter()
        .flat_map(|(class_name, methods)| {
            methods
                .iter()
                .map(|method| (method.clone(), class_name.clone()))
                .collect::<Vec<_>>()
        })
        .collect::<HashMap<_, _>>();

    let class_methods_in_order = methods_by_class
        .values()
        .flat_map(|methods| methods.iter().cloned())
        .collect::<Vec<_>>();

    let mut out = Vec::new();

    for method_name in class_methods_in_order {
        let Some(class_name) = class_for_method.get(&method_name).cloned() else {
            continue;
        };
        let method_sym = Symbol::intern(method_name.as_str());
        let signatures = ts
            .env
            .lookup(&method_sym)
            .unwrap_or_default()
            .iter()
            .map(format_scheme)
            .collect::<Vec<_>>();
        let implemented_on = ts
            .classes
            .instances
            .get(&Symbol::intern(class_name.as_str()))
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|instance| format_instance_target(&class_name, instance))
            .collect::<Vec<_>>();
        out.push(FunctionDoc {
            name: method_name,
            signatures,
            class: Some(class_name),
            implemented_on,
        });
    }

    let class_method_names = out
        .iter()
        .map(|doc| doc.name.clone())
        .collect::<BTreeSet<_>>();

    let mut other_names = ts
        .env
        .values
        .iter()
        .map(|(name, _)| name.to_string())
        .filter(|name| !class_method_names.contains(name))
        .filter(|name| !name.starts_with("prim_"))
        .filter(|name| !primitive_type_names.contains(name))
        .collect::<Vec<_>>();
    other_names.sort();

    for name in other_names {
        let name_sym = Symbol::intern(name.as_str());
        let signatures = ts
            .env
            .lookup(&name_sym)
            .unwrap_or_default()
            .iter()
            .map(format_scheme)
            .collect::<Vec<_>>();
        out.push(FunctionDoc {
            name,
            signatures,
            class: None,
            implemented_on: Vec::new(),
        });
    }

    out
}

fn validate_function_sections(functions: &[FunctionDoc]) -> Result<(), String> {
    let actual = functions
        .iter()
        .map(|function| function.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut categorized = BTreeSet::new();
    let mut duplicates = BTreeSet::new();

    for section in FUNCTION_SECTIONS {
        for name in section.functions {
            if !categorized.insert(*name) {
                duplicates.insert(*name);
            }
        }
    }

    let missing = actual.difference(&categorized).copied().collect::<Vec<_>>();
    let unknown = categorized.difference(&actual).copied().collect::<Vec<_>>();
    if duplicates.is_empty() && missing.is_empty() && unknown.is_empty() {
        return Ok(());
    }

    let mut problems = Vec::new();
    if !duplicates.is_empty() {
        problems.push(format!(
            "functions assigned to multiple sections: {}",
            duplicates.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if !missing.is_empty() {
        problems.push(format!(
            "functions missing from sections: {}",
            missing.join(", ")
        ));
    }
    if !unknown.is_empty() {
        problems.push(format!(
            "section entries that are not built-ins: {}",
            unknown.join(", ")
        ));
    }
    Err(problems.join("\n"))
}

fn required_description_keys(types: &[TypeDoc], functions: &[FunctionDoc]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for t in types {
        out.insert(format!("type:{}", t.name));
    }
    for f in functions {
        out.insert(format!("fn:{}", f.name));
    }
    out
}

fn doc_entry<'a>(
    descriptions: &'a HashMap<String, DocEntry>,
    key: &str,
) -> Result<&'a DocEntry, String> {
    descriptions
        .get(key)
        .ok_or_else(|| format!("missing description for `{key}`"))
}

fn elide_constraints(signature: &str) -> &str {
    match signature.split_once("=>") {
        Some((_, main)) => main.trim(),
        None => signature,
    }
}

fn enclosing_parentheses_cover(text: &str) -> bool {
    if !text.starts_with('(') || !text.ends_with(')') {
        return false;
    }
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
                if depth == 0 && index + ch.len_utf8() != text.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn contains_top_level_comma(text: &str) -> bool {
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

fn strip_redundant_parentheses(mut text: &str) -> &str {
    text = text.trim();
    while enclosing_parentheses_cover(text) {
        let inner = text[1..text.len() - 1].trim();
        if contains_top_level_comma(inner) {
            break;
        }
        text = inner;
    }
    text
}

fn top_level_arrow(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'-' if bytes[index + 1] == b'>' && depth == 0 => return Some(index),
            _ => {}
        }
        index += 1;
    }
    None
}

fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

fn simplify_type(text: &str) -> String {
    let text = strip_redundant_parentheses(text);
    if enclosing_parentheses_cover(text) {
        let inner = text[1..text.len() - 1].trim();
        if contains_top_level_comma(inner) {
            let elements = split_top_level_commas(inner)
                .into_iter()
                .map(simplify_type)
                .collect::<Vec<_>>()
                .join(", ");
            return format!("({elements})");
        }
    }
    let Some(arrow) = top_level_arrow(text) else {
        return text.to_string();
    };

    let left = simplify_type(&text[..arrow]);
    let right = simplify_type(&text[arrow + 2..]);
    let left = if top_level_arrow(strip_redundant_parentheses(&left)).is_some() {
        format!("({left})")
    } else {
        left
    };
    format!("{left} -> {right}")
}

fn simplify_signature(signature: &str, omit_constraints: bool) -> String {
    let signature = if omit_constraints {
        elide_constraints(signature)
    } else {
        signature
    };
    match signature.split_once("=>") {
        Some((constraints, typ)) => {
            format!("{} => {}", constraints.trim(), simplify_type(typ))
        }
        None => simplify_type(signature),
    }
}

fn manual_availability(name: &str) -> Option<&'static str> {
    match name {
        "sum" => Some(
            "`List 'a` and `Option 'a`, where `'a` is a numeric type, `String`, or another `List` type",
        ),
        "mean" => Some("`List f32`, `List f64`, `Option f32`, and `Option f64`"),
        "min" | "max" => {
            Some("`List 'a` and `Option 'a`, where `'a` is a numeric type, `Char`, or `String`")
        }
        _ => None,
    }
}

fn render_markdown(
    types: &[TypeDoc],
    functions: &[FunctionDoc],
    descriptions: &HashMap<String, DocEntry>,
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("# Built-in types & functions\n\n");
    out.push_str(
        "> This page is auto-generated from the prelude source. Run `cargo run -p rex-cli --bin gen_prelude_docs` to refresh it.\n\n",
    );

    out.push_str("## Built-in Types\n\n");
    out.push_str("| Type | Description |\n");
    out.push_str("|---|---|\n");
    for typ in types {
        let key = format!("type:{}", typ.name);
        let mut detail = doc_entry(descriptions, &key)?.description.clone();
        if !typ.constructors.is_empty() {
            let constructors = typ
                .constructors
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = write!(&mut detail, " Constructors: {constructors}.");
        }
        let head = format!("`{}`", format_type_head(&typ.name, typ.arity));
        let _ = writeln!(&mut out, "| {head} | {detail} |");
    }

    out.push_str("\n## Reading Function Entries\n\n");
    out.push_str("Rex functions are curried, so `f first second` can be partially applied as `f first`. The **Call** paragraph gives every parameter a stable, descriptive name. The **Type** paragraph gives the inferred Rex type; type variables begin with an apostrophe. For overloaded functions, **Available for** lists the built-in types and type constructors that provide the operation.\n\n");

    for section in FUNCTION_SECTIONS {
        let _ = writeln!(&mut out, "## {}\n", section.title);
        let _ = writeln!(&mut out, "{}\n", section.introduction);

        for name in section.functions {
            let function = functions
                .iter()
                .find(|function| function.name == *name)
                .ok_or_else(|| format!("missing function metadata for `{name}`"))?;
            let entry = doc_entry(descriptions, &format!("fn:{name}"))?;
            let call = entry
                .call
                .as_deref()
                .ok_or_else(|| format!("missing call form for `fn:{name}`"))?;

            let _ = writeln!(&mut out, "### `{name}`\n");
            let _ = writeln!(&mut out, "**Call:** `{call}`\n");

            let label = if function.signatures.len() == 1 {
                "Type"
            } else {
                "Types"
            };
            let signatures = function
                .signatures
                .iter()
                .map(|signature| {
                    format!(
                        "`{}`",
                        simplify_signature(
                            signature,
                            function.class.is_some() || manual_availability(name).is_some(),
                        )
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let _ = writeln!(&mut out, "**{label}:** {signatures}\n");

            if function.class.is_some() {
                let implementations = function
                    .implemented_on
                    .iter()
                    .map(|typ| format!("`{}`", simplify_type(typ)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(&mut out, "**Available for:** {implementations}\n");
            } else if let Some(availability) = manual_availability(name) {
                let _ = writeln!(&mut out, "**Available for:** {availability}\n");
            }

            let _ = writeln!(&mut out, "{}\n", entry.description);
        }
    }

    out.truncate(out.trim_end().len());
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{simplify_signature, simplify_type};

    #[test]
    fn simplifies_right_associative_function_types() {
        assert_eq!(
            simplify_type("(String -> (String -> (Option u64)))"),
            "String -> String -> Option u64"
        );
        assert_eq!(
            simplify_type("(('a -> Bool) -> ((List 'a) -> (List 'a)))"),
            "('a -> Bool) -> List 'a -> List 'a"
        );
        assert_eq!(simplify_type("(String, 'a)"), "(String, 'a)");
        assert_eq!(simplify_type("(('f 'a), ('f 'b))"), "('f 'a, 'f 'b)");
    }

    #[test]
    fn preserves_or_elides_constraints_as_requested() {
        let signature = "Foldable 'f, Ord 'a => (('f 'a) -> 'a)";
        assert_eq!(
            simplify_signature(signature, false),
            "Foldable 'f, Ord 'a => 'f 'a -> 'a"
        );
        assert_eq!(simplify_signature(signature, true), "'f 'a -> 'a");
    }
}
