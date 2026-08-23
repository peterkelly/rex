//! JSON protocol shared by independently installed Rex tool binaries and the workflow runtime.

use crate::workflow::{
    executor::{OciImage, OciPlatform},
    state::State,
};
use crate::{
    engine::{Builder, CompileOptions, EngineError, Module, ModuleId, virtual_export_name},
    json::{json_to_rex, rex_to_json},
    modules::std::{artifacts, storage::storage_module},
    parser::parse,
    typesystem::{Scheme, Type, TypeBundle, TypeKind, WireType},
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io::Read,
    path::PathBuf,
};

pub const TOOL_PROTOCOL_VERSION: u32 = 1;

/// Build filesystem-CAS/Docker state for an independently installed tool.
pub fn default_tool_state(
    tool_name: &str,
    image_environment_variable: &str,
    default_image: &str,
) -> Result<State, Box<dyn Error>> {
    let store_path = std::env::var_os("REX_STORE")
        .map(PathBuf::from)
        .ok_or("REX_STORE must name the shared content-addressed store directory")?;
    let reference =
        std::env::var(image_environment_variable).unwrap_or_else(|_| default_image.to_owned());
    let image = OciImage::new(tool_name, reference, OciPlatform::native_linux());
    image.validate(true)?;
    Ok(State::docker(
        crate::storage::Store::new_with_filesystem(store_path),
        image,
        true,
    ))
}

/// Description printed by `rex-tool-NAME manifest`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolManifest {
    pub protocol_version: u32,
    pub module: String,
    pub type_bundle: TypeBundle,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defaults: Vec<ToolDefault>,
}

/// One concrete host-backed `Default` value exposed by a tool module.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefault {
    #[serde(rename = "type")]
    pub typ: WireType,
    pub value: JsonValue,
}

impl ToolManifest {
    fn from_module(module: &Module<State>) -> Result<Self, EngineError> {
        Ok(Self {
            protocol_version: TOOL_PROTOCOL_VERSION,
            module: module.name().to_owned(),
            type_bundle: module.type_bundle()?,
            defaults: Vec::new(),
        })
    }

    pub async fn from_factory<M>(module_factory: M) -> Result<Self, Box<dyn Error>>
    where
        M: Fn() -> Result<Module<State>, EngineError> + Copy,
    {
        let module = module_factory()?;
        let mut manifest = Self::from_module(&module)?;
        let default_types = module.default_types().to_vec();
        if default_types.is_empty() {
            return Ok(manifest);
        }
        let module_name = module.name().to_owned();
        drop(module);

        let mut builder =
            Builder::with_prelude(State::without_tools(crate::storage::Store::new_in_memory()))?;
        builder.inject_module(artifacts::module()?)?;
        builder.inject_module(storage_module()?)?;
        builder.inject_module(module_factory()?)?;
        let mut bindings = Vec::new();
        for (index, typ) in default_types.iter().enumerate() {
            let type_name = local_concrete_type_name(typ)?;
            bindings.push(format!(
                "__rex_tool_default_{index}: Tool.{type_name} = default"
            ));
        }
        let result = if bindings.len() == 1 {
            "__rex_tool_default_0".to_owned()
        } else {
            format!(
                "({})",
                (0..bindings.len())
                    .map(|index| format!("__rex_tool_default_{index}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let source = format!(
            "import {module_name} as Tool;\nlet\n    {}\nin\n    {result}",
            bindings.join(",\n    ")
        );
        let program = parse(&source)
            .map_err(|errors| format!("build tool defaults invocation: {errors:?}"))?;
        let compiler = builder.build_compiler();
        let options = CompileOptions::new(ModuleId::parse("rex.tool.defaults")?);
        let (compiled, evaluator) = compiler.compile_program(&program, options).await?;
        let result_type = compiled.result_type().clone();
        let type_system = evaluator.type_system();
        let value = evaluator.run(compiled, BTreeMap::new()).await?;
        let json = rex_to_json(&value, &result_type, type_system.as_ref())?;
        let values = if default_types.len() == 1 {
            vec![json]
        } else {
            json.as_array()
                .cloned()
                .ok_or("default tuple did not encode as a JSON array")?
        };
        manifest.defaults = default_types
            .iter()
            .zip(values)
            .map(|(typ, value)| ToolDefault {
                typ: WireType::from(typ),
                value,
            })
            .collect();
        Ok(manifest)
    }
}

/// Implement the required `manifest` and `execute` commands for a Rust tool crate.
///
/// `execute FUNCTION JSON` accepts either a JSON object keyed by manifest parameter name or a
/// positional JSON array. If JSON is omitted, it is read from standard input.
pub async fn run_tool_cli<M, S>(module_factory: M, state_factory: S) -> Result<(), Box<dyn Error>>
where
    M: Fn() -> Result<Module<State>, EngineError> + Copy,
    S: FnOnce() -> Result<State, Box<dyn Error>>,
{
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("manifest") => {
            if arguments.next().is_some() {
                return Err("manifest takes no arguments".into());
            }
            let manifest = ToolManifest::from_factory(module_factory).await?;
            println!("{}", serde_json::to_string_pretty(&manifest)?);
            Ok(())
        }
        Some("execute") => {
            let function = arguments.next().ok_or("execute requires a function name")?;
            let raw = match arguments.next() {
                Some(raw) => raw,
                None => {
                    let mut raw = String::new();
                    std::io::stdin().read_to_string(&mut raw)?;
                    raw
                }
            };
            if arguments.next().is_some() {
                return Err("execute accepts one JSON argument document".into());
            }
            let arguments = serde_json::from_str(&raw)
                .map_err(|error| format!("invalid argument JSON: {error}"))?;
            let result =
                execute_module_function(module_factory()?, state_factory()?, &function, arguments)
                    .await?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        Some(command) => {
            Err(format!("unknown command `{command}`; expected manifest or execute").into())
        }
        None => Err("expected manifest or execute command".into()),
    }
}

/// Parse and typecheck every `.rex` file in a tool crate's examples directory.
pub async fn typecheck_tool_examples<M>(
    module_factory: M,
    examples: &std::path::Path,
) -> Result<(), Box<dyn Error>>
where
    M: Fn() -> Result<Module<State>, EngineError> + Copy,
{
    let module = module_factory()?;
    if module.docs().is_none_or(|docs| docs.trim().is_empty()) {
        return Err(format!("tool module `{}` has no documentation", module.name()).into());
    }
    for export in module.exports() {
        if export.docs().is_none_or(|docs| docs.trim().is_empty()) {
            return Err(format!("tool export `{}` has no documentation", export.name).into());
        }
        if export
            .params()
            .any(|parameter| parameter.starts_with("arg"))
        {
            return Err(format!(
                "tool export `{}` has a generated parameter name",
                export.name
            )
            .into());
        }
    }
    for staged in module.adts() {
        if staged
            .adt
            .docs
            .as_deref()
            .is_none_or(|docs| docs.trim().is_empty())
        {
            return Err(format!("tool ADT `{}` has no documentation", staged.adt.name).into());
        }
    }
    drop(module);

    let mut paths = std::fs::read_dir(examples)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|extension| extension == "rex"));
    paths.sort();
    for path in paths {
        let source = std::fs::read_to_string(&path)?;
        let program = parse(&source)
            .map_err(|errors| format!("{} did not parse: {errors:?}", path.display()))?;
        let mut builder =
            Builder::with_prelude(State::without_tools(crate::storage::Store::new_in_memory()))?;
        builder.inject_module(artifacts::module()?)?;
        builder.inject_module(storage_module()?)?;
        builder.inject_module(module_factory()?)?;
        let compiler = builder.build_compiler();
        let options = CompileOptions::new(ModuleId::parse("rex.tool.example")?);
        compiler
            .compile_program(&program, options)
            .await
            .map_err(|error| format!("{} did not compile: {error}", path.display()))?;
    }
    Ok(())
}

async fn execute_module_function(
    module: Module<State>,
    state: State,
    function: &str,
    arguments: JsonValue,
) -> Result<JsonValue, Box<dyn Error>> {
    let module_name = module.name().to_owned();
    let bundle = module.type_bundle()?;
    let local_types = bundle
        .adts
        .iter()
        .map(|adt| adt.name.clone())
        .collect::<BTreeSet<_>>();
    let declarations = bundle
        .values
        .get(function)
        .ok_or_else(|| format!("module `{module_name}` does not export `{function}`"))?;
    let [declaration] = declarations.as_slice() else {
        return Err(format!(
            "tool protocol cannot execute overloaded export `{function}` without a concrete type"
        )
        .into());
    };
    let scheme = Scheme::try_from(&declaration.scheme)?;
    if !scheme.vars.is_empty() || !scheme.preds.is_empty() {
        return Err(format!("tool export `{function}` must have a concrete JSON boundary").into());
    }
    let qualified_scheme_type = qualify_tool_type(&scheme.typ, &module_name, &local_types);
    let (parameter_types, result_type) = split_function_type(&qualified_scheme_type);
    let parameter_names = if declaration.params.is_empty() {
        (0..parameter_types.len())
            .map(|index| format!("arg{index}"))
            .collect::<Vec<_>>()
    } else {
        declaration.params.clone()
    };
    let json_arguments = normalize_arguments(arguments, &parameter_names)?;

    let mut builder = Builder::with_prelude(state)?;
    builder.inject_module(artifacts::module()?)?;
    builder.inject_module(storage_module()?)?;
    builder.inject_module(module)?;

    let mut inputs = Module::global();
    for (index, ((json, typ), name)) in json_arguments
        .iter()
        .zip(&parameter_types)
        .zip(&parameter_names)
        .enumerate()
    {
        let value = json_to_rex(json, typ, builder.type_system())
            .map_err(|error| format!("invalid argument `{name}`: {error}"))?;
        let input_name = format!("__rex_tool_argument_{index}");
        inputs.export_native(
            input_name,
            Scheme::new(vec![], vec![], typ.clone()),
            0,
            move |_context, _type, _arguments| Ok(value.clone()),
        )?;
    }
    builder.inject_module(inputs)?;

    let applied_arguments = (0..parameter_types.len())
        .map(|index| format!(" __rex_tool_argument_{index}"))
        .collect::<String>();
    let source = format!("import {module_name} ({function});\n{function}{applied_arguments}");
    let program = parse(&source).map_err(|errors| format!("build tool invocation: {errors:?}"))?;
    let options = CompileOptions::new(ModuleId::parse("rex.tool.execute")?);
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler.compile_program(&program, options).await?;
    let type_system = evaluator.type_system();
    let value = evaluator.run(compiled, BTreeMap::new()).await?;
    rex_to_json(&value, &result_type, type_system.as_ref())
        .map_err(|error| format!("encode `{function}` result: {error}").into())
}

fn normalize_arguments(
    arguments: JsonValue,
    parameter_names: &[String],
) -> Result<Vec<JsonValue>, Box<dyn Error>> {
    match arguments {
        JsonValue::Array(values) if values.len() == parameter_names.len() => Ok(values),
        JsonValue::Array(values) => Err(format!(
            "expected {} arguments, got {}",
            parameter_names.len(),
            values.len()
        )
        .into()),
        JsonValue::Object(mut values) => {
            let mut ordered = Vec::with_capacity(parameter_names.len());
            for name in parameter_names {
                ordered.push(
                    values
                        .remove(name)
                        .ok_or_else(|| format!("missing argument `{name}`"))?,
                );
            }
            if !values.is_empty() {
                return Err(format!(
                    "unexpected argument(s): {}",
                    values.keys().cloned().collect::<Vec<_>>().join(", ")
                )
                .into());
            }
            Ok(ordered)
        }
        value => Err(format!("arguments must be a JSON object or array, got {value}").into()),
    }
}

fn split_function_type(typ: &Type) -> (Vec<Type>, Type) {
    let mut parameters = Vec::new();
    let mut current = typ.clone();
    while let TypeKind::Fun(parameter, result) = current.as_ref() {
        parameters.push(parameter.clone());
        current = result.clone();
    }
    (parameters, current)
}

fn local_concrete_type_name(typ: &Type) -> Result<String, Box<dyn Error>> {
    match typ.as_ref() {
        TypeKind::Con(con) if con.arity() == 0 => Ok(con.name().to_string()),
        _ => Err(format!("default type `{typ}` is not a concrete nullary type constructor").into()),
    }
}

fn qualify_tool_type(typ: &Type, module_name: &str, local_types: &BTreeSet<String>) -> Type {
    match typ.as_ref() {
        TypeKind::Con(con) => match con.user_name() {
            Some(name) if local_types.contains(name.as_ref()) => {
                Type::con(virtual_export_name(module_name, name.as_ref()), con.arity())
            }
            _ => typ.clone(),
        },
        TypeKind::App(function, argument) => Type::app(
            qualify_tool_type(function, module_name, local_types),
            qualify_tool_type(argument, module_name, local_types),
        ),
        TypeKind::Fun(argument, result) => Type::fun(
            qualify_tool_type(argument, module_name, local_types),
            qualify_tool_type(result, module_name, local_types),
        ),
        TypeKind::Tuple(items) => Type::tuple(
            items
                .iter()
                .map(|item| qualify_tool_type(item, module_name, local_types)),
        ),
        TypeKind::Record(fields) => Type::new(TypeKind::Record(
            fields
                .iter()
                .map(|(name, field)| {
                    (
                        name.clone(),
                        qualify_tool_type(field, module_name, local_types),
                    )
                })
                .collect(),
        )),
        TypeKind::Var(_) => typ.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Export;
    use serde_json::json;

    #[derive(Clone, Debug, Default, PartialEq, rex::Rex)]
    struct Settings {
        count: i32,
    }

    fn test_module() -> Result<Module<State>, EngineError> {
        let mut module = Module::new(
            "tools.protocol_test",
            Some("Protocol test module.".to_owned()),
        );
        module.add_rex_adt::<Settings>()?;
        module.add_rex_default_instance::<Settings>()?;
        let export = Export::from_handler("increment", |_state: State, value: i32| {
            Ok::<i32, EngineError>(value + 1)
        })?
        .with_param_names(["value"])?
        .with_docs("Increment an integer.");
        module.add_export(export)?;
        Ok(module)
    }

    #[tokio::test]
    async fn manifest_preserves_docs_types_parameters_and_defaults() {
        let manifest = ToolManifest::from_factory(test_module).await.unwrap();
        assert_eq!(manifest.protocol_version, TOOL_PROTOCOL_VERSION);
        assert_eq!(manifest.module, "tools.protocol_test");
        assert_eq!(
            manifest.type_bundle.docs.as_deref(),
            Some("Protocol test module.")
        );
        let increment = &manifest.type_bundle.values["increment"][0];
        assert_eq!(increment.params, ["value"]);
        assert_eq!(increment.docs.as_deref(), Some("Increment an integer."));
        assert_eq!(manifest.type_bundle.adts[0].name, "Settings");
        assert_eq!(manifest.defaults.len(), 1);
        assert_eq!(manifest.defaults[0].value, json!({ "count": 0 }));
    }

    #[tokio::test]
    async fn execute_uses_the_typed_json_boundary() {
        let result = execute_module_function(
            test_module().unwrap(),
            State::without_tools(crate::storage::Store::new_in_memory()),
            "increment",
            json!({ "value": 41 }),
        )
        .await
        .unwrap();
        assert_eq!(result, json!(42));
    }
}
