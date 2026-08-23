use crate::workflow::{
    state::{ExternalTools, State},
    tool_protocol::{TOOL_PROTOCOL_VERSION, ToolManifest},
};
use crate::{
    engine::{
        Context, EngineError, Export, ImportRequest, Importer, Module, NativeFuture,
        ResolvedModule, ResolvedModuleContent, Value, virtual_export_name,
    },
    json::{json_to_rex, rex_to_json},
    typesystem::{Type, TypeKind},
};
use futures::{FutureExt, future::BoxFuture};
use serde_json::{Map, Value as JsonValue};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{io::AsyncWriteExt, process::Command};

pub struct ExternalToolImporter {
    config: ExternalTools,
}

impl ExternalToolImporter {
    pub fn new(config: ExternalTools) -> Self {
        Self { config }
    }
}

impl Importer<State> for ExternalToolImporter {
    fn import<'a>(
        &'a self,
        request: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule<State>>, EngineError>> {
        Box::pin(async move {
            let requested = request.module_id.to_string();
            let Some(name) = requested.strip_prefix("tools.") else {
                return Ok(None);
            };
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Ok(None);
            }
            let executable = tool_executable(&self.config.directory, name);
            if !executable.is_file() {
                return Ok(None);
            }
            let manifest = read_manifest(&executable, &self.config).await?;
            if manifest.protocol_version != TOOL_PROTOCOL_VERSION {
                return Err(EngineError::Custom(format!(
                    "tool `{}` uses protocol version {}, but the Rex workflow runtime supports {}",
                    executable.display(),
                    manifest.protocol_version,
                    TOOL_PROTOCOL_VERSION
                )));
            }
            if manifest.module != requested {
                return Err(EngineError::Custom(format!(
                    "tool `{}` declares module `{}` instead of requested module `{requested}`",
                    executable.display(),
                    manifest.module
                )));
            }
            let module = module_from_manifest(manifest, executable, self.config.clone())?;
            Ok(Some(ResolvedModule {
                id: request.module_id,
                content: ResolvedModuleContent::module(module),
            }))
        })
    }
}

fn tool_executable(directory: &Path, name: &str) -> PathBuf {
    let suffix = std::env::consts::EXE_SUFFIX;
    directory.join(format!("rex-tool-{name}{suffix}"))
}

async fn read_manifest(
    executable: &Path,
    config: &ExternalTools,
) -> Result<ToolManifest, EngineError> {
    let output = Command::new(executable)
        .arg("manifest")
        .envs(&config.environment)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| {
            EngineError::Custom(format!("run `{}` manifest: {error}", executable.display()))
        })?;
    if !output.status.success() {
        return Err(EngineError::Custom(format!(
            "`{} manifest` failed with status {}: {}",
            executable.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        EngineError::Custom(format!(
            "parse manifest from `{}`: {error}",
            executable.display()
        ))
    })
}

fn module_from_manifest(
    manifest: ToolManifest,
    executable: PathBuf,
    config: ExternalTools,
) -> Result<Module<State>, EngineError> {
    let defaults = manifest.defaults;
    let parts = manifest
        .type_bundle
        .into_parts()
        .map_err(EngineError::Type)?;
    let module_name = manifest.module;
    let local_types = parts
        .adts
        .iter()
        .map(|adt| adt.name.to_string())
        .collect::<BTreeSet<_>>();
    let mut module = Module::new(&module_name, parts.docs);
    module.add_adt_family(parts.adts)?;
    for default in defaults {
        let typ = Type::try_from(default.typ).map_err(EngineError::Type)?;
        let decode_type = qualify_tool_type(&typ, &module_name, &local_types);
        let value = default.value;
        module.add_native_default_instance(typ, move |context, _call_type| {
            json_to_rex(&value, &decode_type, context.type_system()).map_err(|error| {
                EngineError::Custom(format!("decode installed tool default: {error}"))
            })
        })?;
    }
    for (name, declarations) in parts.values {
        let [declaration] = declarations.as_slice() else {
            return Err(EngineError::Custom(format!(
                "installed tool export `{name}` is overloaded; protocol v1 requires one declaration"
            )));
        };
        if !declaration.scheme.vars.is_empty() || !declaration.scheme.preds.is_empty() {
            return Err(EngineError::Custom(format!(
                "installed tool export `{name}` must have a concrete JSON boundary"
            )));
        }
        let qualified_type = qualify_tool_type(&declaration.scheme.typ, &module_name, &local_types);
        let (parameter_types, result_type) = split_function_type(&qualified_type);
        let parameter_names = if declaration.params.is_empty() {
            (0..parameter_types.len())
                .map(|index| format!("arg{index}"))
                .collect::<Vec<_>>()
        } else {
            declaration.params.iter().map(ToString::to_string).collect()
        };
        let function = name.clone();
        let tool = executable.clone();
        let tool_config = config.clone();
        let scheme = declaration.scheme.clone();
        let docs = declaration.docs.clone();
        let export = Export::from_native_async(
            name,
            scheme,
            parameter_types.len(),
            move |context: Context<State>, _call_type: Type, arguments: Vec<Value>| -> NativeFuture {
                let function = function.clone();
                let tool = tool.clone();
                let tool_config = tool_config.clone();
                let parameter_types = parameter_types.clone();
                let parameter_names = parameter_names.clone();
                let result_type = result_type.clone();
                async move {
                    let mut json = Map::new();
                    for (((argument, typ), parameter), index) in arguments
                        .iter()
                        .zip(&parameter_types)
                        .zip(&parameter_names)
                        .zip(0usize..)
                    {
                        let value = rex_to_json(argument, typ, context.type_system()).map_err(|error| {
                            EngineError::Custom(format!(
                                "encode argument {index} (`{parameter}`) for `{function}`: {error}"
                            ))
                        })?;
                        json.insert(parameter.clone(), value);
                    }
                    let result = execute_tool(
                        &tool,
                        &tool_config,
                        &function,
                        JsonValue::Object(json),
                    )
                    .await?;
                    json_to_rex(&result, &result_type, context.type_system()).map_err(|error| {
                        EngineError::Custom(format!("decode result from `{function}`: {error}"))
                    })
                }
                .boxed()
            },
        )?
        .with_param_names(declaration.params.iter().map(ToString::to_string))?;
        let export = match docs {
            Some(docs) => export.with_docs(docs),
            None => export,
        };
        module.add_export(export)?;
    }
    Ok(module)
}

async fn execute_tool(
    executable: &Path,
    config: &ExternalTools,
    function: &str,
    arguments: JsonValue,
) -> Result<JsonValue, EngineError> {
    let mut child = Command::new(executable)
        .args(["execute", function])
        .envs(&config.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| EngineError::Custom(format!("run `{}`: {error}", executable.display())))?;
    let input = serde_json::to_vec(&arguments)
        .map_err(|error| EngineError::Custom(format!("encode tool arguments: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| EngineError::Custom(format!("open stdin for `{}`", executable.display())))?;
    stdin.write_all(&input).await.map_err(|error| {
        EngineError::Custom(format!("write `{}` stdin: {error}", executable.display()))
    })?;
    drop(stdin);
    let output = child.wait_with_output().await.map_err(|error| {
        EngineError::Custom(format!("wait for `{}`: {error}", executable.display()))
    })?;
    if !output.status.success() {
        return Err(EngineError::Custom(format!(
            "tool `{function}` failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        EngineError::Custom(format!("parse result from tool `{function}`: {error}"))
    })
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
