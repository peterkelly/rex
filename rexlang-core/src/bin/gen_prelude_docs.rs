#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use rex_ast::expr::{Decl, Symbol};
use rex_ts::{Predicate, Scheme, Type, TypeKind, TypeSystem, prelude_typeclasses_program};

const OUTPUT_PATH: &str = "docs/src/PRELUDE.md";
const DESCRIPTIONS_PATH: &str = "docs/src/prelude_descriptions.txt";
const TYPE_SIGNATURE_MAX: usize = 30;

#[derive(Clone, Debug)]
struct TypeDoc {
    name: String,
    arity: usize,
    constructors: Vec<String>,
}

#[derive(Clone, Debug)]
struct ClassDoc {
    name: String,
    supers: Vec<String>,
    methods: Vec<String>,
}

#[derive(Clone, Debug)]
struct FunctionDoc {
    name: String,
    signatures: Vec<String>,
    class: Option<String>,
    implemented_on: Vec<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let descriptions = load_descriptions(Path::new(DESCRIPTIONS_PATH))?;
    let ts = TypeSystem::with_prelude().map_err(|e| format!("{e}"))?;
    let program = prelude_typeclasses_program().map_err(|e| format!("{e}"))?;

    let mut type_arity = BTreeMap::<String, usize>::new();
    collect_all_type_constructors(&ts, &mut type_arity);

    let primitive_type_names = collect_primitive_type_names(&ts);
    let methods_by_class = collect_methods_by_class(program)?;
    let classes = build_classes(&ts, &methods_by_class)?;
    let types = build_types(&ts, &type_arity);
    let functions = build_functions(&ts, &methods_by_class, &primitive_type_names);

    let required_keys = required_description_keys(&types, &classes, &functions);
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

    let markdown = render_markdown(&types, &classes, &functions, &descriptions)?;
    fs::write(OUTPUT_PATH, markdown).map_err(|e| format!("failed to write {OUTPUT_PATH}: {e}"))?;
    println!("wrote {OUTPUT_PATH}");
    Ok(())
}

fn load_descriptions(path: &Path) -> Result<HashMap<String, String>, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut descriptions = HashMap::new();
    for (line_no, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, description) = line.split_once('\t').ok_or_else(|| {
            format!(
                "{}:{}: expected `key<TAB>description`",
                path.display(),
                line_no + 1
            )
        })?;
        let key = key.trim().to_string();
        let description = description.trim().to_string();
        if key.is_empty() || description.is_empty() {
            return Err(format!(
                "{}:{}: key and description must be non-empty",
                path.display(),
                line_no + 1
            ));
        }
        if descriptions.insert(key.clone(), description).is_some() {
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
            && c.arity == 0
            && c.name == *name
        {
            out.insert(name.to_string());
        }
    }
    out
}

fn collect_methods_by_class(
    program: &rex_ast::expr::Program,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut out = BTreeMap::<String, Vec<String>>::new();
    for decl in &program.decls {
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
            out.entry(c.name.to_string())
                .and_modify(|arity| *arity = (*arity).max(c.arity))
                .or_insert(c.arity);
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

fn build_classes(
    ts: &TypeSystem,
    methods_by_class: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<ClassDoc>, String> {
    let mut out = Vec::new();
    for (class_name, methods) in methods_by_class {
        let info = ts
            .class_info
            .get(&Symbol::from(class_name.as_str()))
            .ok_or_else(|| format!("missing class info for `{class_name}`"))?;
        let supers = info.supers.iter().map(ToString::to_string).collect();
        out.push(ClassDoc {
            name: class_name.clone(),
            supers,
            methods: methods.clone(),
        });
    }
    Ok(out)
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

fn format_instance_head(inst: &rex_ts::Instance) -> String {
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
        let method_sym = Symbol::from(method_name.as_str());
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
            .get(&Symbol::from(class_name.as_str()))
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(format_instance_head)
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
        let sym = Symbol::from(name.as_str());
        let signatures = ts
            .env
            .lookup(&sym)
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

fn required_description_keys(
    types: &[TypeDoc],
    classes: &[ClassDoc],
    functions: &[FunctionDoc],
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for t in types {
        out.insert(format!("type:{}", t.name));
    }
    for c in classes {
        out.insert(format!("class:{}", c.name));
    }
    for f in functions {
        out.insert(format!("fn:{}", f.name));
    }
    out
}

fn desc<'a>(descriptions: &'a HashMap<String, String>, key: &str) -> Result<&'a str, String> {
    descriptions
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("missing description for `{key}`"))
}

fn wrap_text(text: &str, max: usize) -> Vec<String> {
    if text.len() <= max {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if word.len() > max {
            if !current.is_empty() {
                lines.push(current.clone());
                current.clear();
            }
            let mut start = 0;
            while start < word.len() {
                let end = (start + max).min(word.len());
                lines.push(word[start..end].to_string());
                start = end;
            }
            continue;
        }

        let candidate_len = if current.is_empty() {
            word.len()
        } else {
            current.len() + 1 + word.len()
        };
        if candidate_len > max && !current.is_empty() {
            lines.push(current.clone());
            current.clear();
        }
        if current.is_empty() {
            current.push_str(word);
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(text.to_string());
    }
    lines
}

fn format_signature_cell(signature: &str) -> String {
    wrap_text(signature, TYPE_SIGNATURE_MAX)
        .into_iter()
        .map(|line| format!("`{line}`"))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn elide_constraints(signature: &str) -> String {
    match signature.split_once("=>") {
        Some((_, main)) => main.trim().to_string(),
        None => signature.to_string(),
    }
}

fn render_markdown(
    types: &[TypeDoc],
    classes: &[ClassDoc],
    functions: &[FunctionDoc],
    descriptions: &HashMap<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("# Built-in types & functions\n\n");
    out.push_str(
        "> This page is auto-generated from the prelude source. Run `cargo run -p rexlang-core --bin gen_prelude_docs` to refresh it.\n\n",
    );

    out.push_str("## Built-in Types\n\n");
    out.push_str("| Type | Description |\n");
    out.push_str("|---|---|\n");
    for typ in types {
        let key = format!("type:{}", typ.name);
        let mut detail = desc(descriptions, &key)?.to_string();
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

    out.push_str("\n## Built-in Type Classes\n\n");
    for class_doc in classes {
        let class_name = &class_doc.name;
        let class_desc = desc(descriptions, &format!("class:{class_name}"))?;
        let _ = writeln!(&mut out, "### `{class_name}`");
        let _ = writeln!(&mut out, "{class_desc}");
        if class_doc.supers.is_empty() {
            out.push_str("\nSuperclasses: _none_\n\n");
        } else {
            let supers = class_doc
                .supers
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(&mut out, "\nSuperclasses: {supers}\n");
        }
        out.push_str("Methods:\n");
        for method in &class_doc.methods {
            let fn_desc = desc(descriptions, &format!("fn:{method}"))?;
            let signature = functions
                .iter()
                .find(|f| &f.name == method)
                .and_then(|f| f.signatures.first())
                .cloned()
                .unwrap_or_else(|| "<missing signature>".to_string());
            let _ = writeln!(&mut out, "- `{method}`: `{signature}`. {fn_desc}",);
        }
        out.push('\n');
    }

    out.push_str("## Built-in Functions\n\n");
    out.push_str("### Overloaded (Type Class Methods)\n\n");
    out.push_str("| Function | Signature | Implemented On | Description |\n");
    out.push_str("|---|---|---|---|\n");
    for function in functions.iter().filter(|f| f.class.is_some()) {
        let implementations = if function.implemented_on.is_empty() {
            "_none_".to_string()
        } else {
            function
                .implemented_on
                .iter()
                .map(|h| format!("`{h}`"))
                .collect::<Vec<_>>()
                .join("<br>")
        };
        let signatures = if function.signatures.is_empty() {
            "_none_".to_string()
        } else {
            function
                .signatures
                .iter()
                .map(|s| format_signature_cell(&elide_constraints(s)))
                .collect::<Vec<_>>()
                .join("<br><br>")
        };
        let fn_desc = desc(descriptions, &format!("fn:{}", function.name))?;
        let _ = writeln!(
            &mut out,
            "| `{}` | {} | {} | {} |",
            function.name, signatures, implementations, fn_desc
        );
    }

    out.push_str("\n### Other Built-ins\n\n");
    out.push_str("| Function | Signature | Description |\n");
    out.push_str("|---|---|---|\n");
    for function in functions.iter().filter(|f| f.class.is_none()) {
        let signatures = if function.signatures.is_empty() {
            "_none_".to_string()
        } else {
            function
                .signatures
                .iter()
                .map(|s| format_signature_cell(s))
                .collect::<Vec<_>>()
                .join("<br><br>")
        };
        let fn_desc = desc(descriptions, &format!("fn:{}", function.name))?;
        let _ = writeln!(
            &mut out,
            "| `{}` | {} | {} |",
            function.name, signatures, fn_desc
        );
    }

    Ok(out)
}
