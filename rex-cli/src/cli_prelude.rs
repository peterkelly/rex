use rand::prelude::*;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rex::{
    Rex,
    engine::{Engine, EngineError, Module},
};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::OnceCell;
use uuid::Uuid;

#[derive(Default)]
struct SubprocessRegistry {
    procs: Mutex<HashMap<Uuid, Arc<SubprocessEntry>>>,
}

struct SubprocessEntry {
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    output: OnceCell<Arc<SubprocessOutput>>,
}

struct SubprocessOutput {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl SubprocessEntry {
    fn new(child: tokio::process::Child) -> Self {
        Self {
            child: tokio::sync::Mutex::new(Some(child)),
            output: OnceCell::new(),
        }
    }
}

static SUBPROCESSES: OnceLock<SubprocessRegistry> = OnceLock::new();

fn subprocess_registry() -> &'static SubprocessRegistry {
    SUBPROCESSES.get_or_init(SubprocessRegistry::default)
}

#[derive(Rex)]
struct SpawnOptions {
    cmd: String,
    args: Vec<String>,
}

#[derive(Clone, Copy, Rex)]
#[rex(name = "Subprocess")]
struct CliSubprocess {
    id: Uuid,
}

pub fn inject_cli_prelude_engine(engine: &mut Engine) -> Result<(), EngineError> {
    inject_cli_test_natives(engine)?;
    inject_cli_io_natives(engine)?;
    inject_cli_process_natives(engine)?;
    Ok(())
}

fn inject_cli_test_natives(engine: &mut Engine) -> Result<(), EngineError> {
    let mut module = Module::new("test");
    module.export_async("do_something", |_state: &(), n: i32| async move {
        println!("do_something {} begin", n);
        let extra = {
            let mut rng = rand::rng();
            rng.random_range(1..=1000)
        };
        tokio::time::sleep(Duration::from_millis(1000 + extra)).await;
        println!("do_something {} end", n);
        Ok::<i32, EngineError>(n)
    })?;
    module.export_async("is_even", |_state: &(), n: i32| async move {
        println!("is_even {} begin", n);
        let extra = {
            let mut rng = rand::rng();
            rng.random_range(1..=1000)
        };
        tokio::time::sleep(Duration::from_millis(1000 + extra)).await;
        println!("is_even {} end", n);
        Ok::<bool, EngineError>(n % 2 == 0)
    })?;
    engine.inject_module(module)
}

fn inject_cli_io_natives(engine: &mut Engine) -> Result<(), EngineError> {
    let mut module = Module::new("std.io");
    module.export("debug", |_state: &(), message: String| {
        tracing::debug!("{message}");
        Ok::<String, EngineError>(message)
    })?;
    module.export("info", |_state: &(), message: String| {
        tracing::info!("{message}");
        Ok::<String, EngineError>(message)
    })?;
    module.export("warn", |_state: &(), message: String| {
        tracing::warn!("{message}");
        Ok::<String, EngineError>(message)
    })?;
    module.export("error", |_state: &(), message: String| {
        tracing::error!("{message}");
        Ok::<String, EngineError>(message)
    })?;

    module.export_async("read_all", |_state: &(), fd: i32| async move {
        if fd != 0 {
            return Err(EngineError::Internal(format!(
                "read_all only supports fd 0 (stdin), got {fd}"
            )));
        }

        let mut buf = Vec::new();
        io::stdin()
            .read_to_end(&mut buf)
            .await
            .map_err(|e| EngineError::Internal(format!("read_all failed: {e}")))?;
        Ok::<Vec<u8>, EngineError>(buf)
    })?;

    module.export_async(
        "write_all",
        |_state: &(), fd: i32, bytes: Vec<u8>| async move {
            match fd {
                1 => {
                    let mut out = io::stdout();
                    out.write_all(&bytes)
                        .await
                        .map_err(|e| EngineError::Internal(format!("write_all failed: {e}")))?;
                    out.flush()
                        .await
                        .map_err(|e| EngineError::Internal(format!("write_all failed: {e}")))?;
                }
                2 => {
                    let mut out = io::stderr();
                    out.write_all(&bytes)
                        .await
                        .map_err(|e| EngineError::Internal(format!("write_all failed: {e}")))?;
                    out.flush()
                        .await
                        .map_err(|e| EngineError::Internal(format!("write_all failed: {e}")))?;
                }
                _ => {
                    return Err(EngineError::Internal(format!(
                        "write_all only supports fd 1 (stdout) and 2 (stderr), got {fd}"
                    )));
                }
            }

            Ok::<(), EngineError>(())
        },
    )?;

    engine.inject_module(module)
}

fn inject_cli_process_natives(engine: &mut Engine) -> Result<(), EngineError> {
    let mut module = Module::new("std.process");
    module.add_rex_adt::<SpawnOptions>()?;
    module.add_rex_adt::<CliSubprocess>()?;

    module.export_async("spawn", |_state: &(), opts: SpawnOptions| async move {
        let child = Command::new(opts.cmd)
            .args(opts.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| EngineError::Internal(format!("spawn failed: {e}")))?;

        let id = Uuid::new_v4();
        let entry = Arc::new(SubprocessEntry::new(child));

        subprocess_registry()
            .procs
            .lock()
            .map_err(|_| {
                EngineError::Internal(
                    "std.process.spawn: subprocess registry mutex poisoned (this is a bug)".into(),
                )
            })?
            .insert(id, entry);

        Ok::<CliSubprocess, EngineError>(CliSubprocess { id })
    })?;

    module.export_async(
        "wait",
        |_state: &(), subprocess: CliSubprocess| async move {
            let entry = subprocess_get(&subprocess.id, "std.process.wait")?;
            let output = subprocess_output(&entry, "std.process.wait").await?;
            Ok::<i32, EngineError>(output.exit_code)
        },
    )?;

    module.export_async(
        "stdout",
        |_state: &(), subprocess: CliSubprocess| async move {
            let entry = subprocess_get(&subprocess.id, "std.process.stdout")?;
            let output = subprocess_output(&entry, "std.process.stdout").await?;
            Ok::<Vec<u8>, EngineError>(output.stdout.clone())
        },
    )?;

    module.export_async(
        "stderr",
        |_state: &(), subprocess: CliSubprocess| async move {
            let entry = subprocess_get(&subprocess.id, "std.process.stderr")?;
            let output = subprocess_output(&entry, "std.process.stderr").await?;
            Ok::<Vec<u8>, EngineError>(output.stderr.clone())
        },
    )?;

    engine.inject_module(module)
}

fn subprocess_get(id: &Uuid, name: &str) -> Result<Arc<SubprocessEntry>, EngineError> {
    subprocess_registry()
        .procs
        .lock()
        .map_err(|_| {
            EngineError::Internal(format!(
                "{name}: subprocess registry mutex poisoned (this is a bug)"
            ))
        })?
        .get(id)
        .cloned()
        .ok_or_else(|| EngineError::Internal(format!("{name}: unknown subprocess id {id}")))
}

async fn subprocess_output(
    entry: &Arc<SubprocessEntry>,
    name: &'static str,
) -> Result<Arc<SubprocessOutput>, EngineError> {
    entry
        .output
        .get_or_try_init(|| async move {
            let child = {
                let mut child_guard = entry.child.lock().await;
                child_guard.take().ok_or_else(|| {
                    EngineError::Internal(format!("{name}: subprocess collection already started"))
                })?
            };
            let output = child
                .wait_with_output()
                .await
                .map_err(|e| EngineError::Internal(format!("{name}: wait failed: {e}")))?;
            Ok::<Arc<SubprocessOutput>, EngineError>(Arc::new(SubprocessOutput {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: output.stdout,
                stderr: output.stderr,
            }))
        })
        .await
        .cloned()
}

#[cfg(test)]
mod tests {
    use rex::{
        engine::{Engine, Value},
        parser::parse as parse_rex,
        typesystem::{BuiltinTypeId, Type},
    };

    use super::*;
    #[tokio::test]
    async fn cli_prelude_typecheck_smoke() {
        let code = r#"
            import std.process;
            import std.io;

            let p = process.spawn (process.SpawnOptions {
                cmd = "sh",
                args = to_array ["-c", "printf hi"]
            }) in
              io.write_all 1 (process.stdout p)
        "#;

        let mut engine = Engine::with_prelude(()).unwrap();

        inject_cli_prelude_engine(&mut engine).unwrap();
        let mut compiler = engine.into_compiler();
        let parsed = parse_rex(code).unwrap();
        let program = compiler
            .compile_program(&parsed, Default::default())
            .await
            .unwrap();
        compiler
            .into_evaluator()
            .run(program, Default::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cli_log_exports_are_string_functions() {
        let code = r#"
            import std.io;

            io.info "hello"
        "#;

        let mut engine = Engine::with_prelude(()).unwrap();

        inject_cli_prelude_engine(&mut engine).unwrap();
        let mut compiler = engine.into_compiler();
        let parsed = parse_rex(code).unwrap();
        let program = compiler
            .compile_program(&parsed, Default::default())
            .await
            .unwrap();
        let ty = program.result_type().clone();
        let value = compiler
            .into_evaluator()
            .run(program, Default::default())
            .await
            .unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
        assert_eq!(value.to_rust::<String>().unwrap(), "hello");
    }

    #[tokio::test]
    async fn cli_subprocess_captures_stdout_and_exit_code() {
        let code = r#"
            import std.process;

            let p = process.spawn (process.SpawnOptions {
                cmd = "sh",
                args = to_array ["-c", "printf hi"]
            }) in
              (process.wait p, process.stdout p, process.stderr p)
        "#;

        let mut engine = Engine::with_prelude(()).unwrap();

        inject_cli_prelude_engine(&mut engine).unwrap();
        let mut compiler = engine.into_compiler();
        let parsed = parse_rex(code).unwrap();
        let program = compiler
            .compile_program(&parsed, Default::default())
            .await
            .unwrap();
        let ty = program.result_type().clone();
        let value = compiler
            .into_evaluator()
            .run(program, Default::default())
            .await
            .unwrap();
        assert_eq!(
            ty,
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::I32),
                Type::array(Type::builtin(BuiltinTypeId::U8)),
                Type::array(Type::builtin(BuiltinTypeId::U8)),
            ])
        );
        let Value::Tuple(xs) = value.value().unwrap() else {
            panic!("expected tuple");
        };
        assert_eq!(xs[0].to_rust::<i32>().unwrap(), 0);

        let Value::Array(out) = xs[1].value().unwrap() else {
            panic!("expected stdout bytes");
        };
        let got: Vec<u8> = out.iter().map(|v| v.to_rust::<u8>().unwrap()).collect();
        assert_eq!(got, b"hi");

        let Value::Array(err) = xs[2].value().unwrap() else {
            panic!("expected stderr bytes");
        };
        assert!(err.is_empty());
    }
}
