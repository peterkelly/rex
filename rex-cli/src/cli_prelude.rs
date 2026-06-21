use rand::prelude::*;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::modules::stdio;
pub use crate::modules::stdio::{io_result_type_arg, run_io_handle};
use rex::{
    Rex,
    engine::{Builder, EngineError, Handle, Module},
    typesystem::{BuiltinTypeId, Scheme, Type},
};
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

pub fn inject_cli_prelude_builder(builder: &mut Builder) -> Result<(), EngineError> {
    inject_cli_test_natives(builder)?;
    stdio::inject_cli_io_natives(builder)?;
    inject_cli_process_natives(builder)?;
    Ok(())
}

fn inject_cli_test_natives(builder: &mut Builder) -> Result<(), EngineError> {
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

    let u64_ty = Type::builtin(BuiltinTypeId::U64);
    let string_list_ty = Type::list(Type::builtin(BuiltinTypeId::String));
    let scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(&u64_ty, Type::fun(&u64_ty, &string_list_ty)),
    );
    module.export_native("make_hybrid_list", scheme, 2, |engine, _scheme, args| {
        let ncons = args[0].as_u64()?;
        let nflat = args[1].as_u64()?;
        let mut flat_contents: Vec<Handle> = Vec::new();
        for i in 0..nflat {
            flat_contents.push(engine.heap().alloc_string(format!("F-{}", ncons + i))?);
        }
        let data = engine.heap().alloc_data(flat_contents)?;
        let mut tail = engine.heap().alloc_list_slice(0, data)?;
        for i in 0..ncons {
            let head = engine.heap().alloc_string(format!("C-{}", ncons + i))?;
            tail = engine.heap().alloc_cons(head, tail)?;
        }
        Ok(tail)
    })?;

    builder.inject_module(module)
}

fn inject_cli_process_natives(builder: &mut Builder) -> Result<(), EngineError> {
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

    builder.inject_module(module)
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
        engine::{Builder, CompileOptions, Value},
        parser::parse as parse_rex,
        typesystem::{BuiltinTypeId, Type},
    };

    use super::*;

    fn compile_options() -> CompileOptions {
        CompileOptions::for_module("cli.test").unwrap()
    }
    #[tokio::test]
    async fn cli_prelude_typecheck_smoke() {
        let code = r#"
            import std.process;
            import std.io;

            let p = process.spawn (process.SpawnOptions {
                cmd = "sh",
                args = ["-c", "printf hi"]
            }) in
              io.write_all 1 (process.stdout p)
        "#;

        let mut builder = Builder::with_prelude(()).unwrap();

        inject_cli_prelude_builder(&mut builder).unwrap();
        let compiler = builder.build_compiler();
        let parsed = parse_rex(code).unwrap();
        let (program, evaluator) = compiler
            .compile_program(&parsed, compile_options())
            .await
            .unwrap();
        let (value, ctx) = evaluator
            .run_with_context(program, Default::default())
            .await
            .unwrap();
        run_io_handle(ctx, value).await.unwrap();
    }

    #[tokio::test]
    async fn cli_log_exports_are_string_functions() {
        let code = r#"
            import std.io;

            io.info "hello"
        "#;

        let mut builder = Builder::with_prelude(()).unwrap();

        inject_cli_prelude_builder(&mut builder).unwrap();
        let compiler = builder.build_compiler();
        let parsed = parse_rex(code).unwrap();
        let (program, evaluator) = compiler
            .compile_program(&parsed, compile_options())
            .await
            .unwrap();
        let ty = program.result_type().clone();
        let (value, ctx) = evaluator
            .run_with_context(program, Default::default())
            .await
            .unwrap();
        let inner = io_result_type_arg(&ty).unwrap();
        let value = run_io_handle(ctx, value).await.unwrap();
        assert_eq!(inner, Type::builtin(BuiltinTypeId::String));
        assert_eq!(value.to_rust::<String>().unwrap(), "hello");
    }

    #[tokio::test]
    async fn cli_io_filesystem_actions_sequence_with_bind() {
        let path = std::env::temp_dir().join(format!("rex-cli-io-{}.txt", Uuid::new_v4()));
        let path_str = path.display().to_string();
        let code = format!(
            r#"
            import std.io;

            let path = "{path_str}" in
              bind (\_ ->
                bind (\contents ->
                  bind (\exists ->
                    bind (\_ -> pure (contents, exists))
                         (io.remove_file path))
                       (io.exists path))
                     (io.read_file path))
                   (io.write_file path "hello")
            "#
        );

        let mut builder = Builder::with_prelude(()).unwrap();

        inject_cli_prelude_builder(&mut builder).unwrap();
        let compiler = builder.build_compiler();
        let parsed = parse_rex(&code).unwrap();
        let (program, evaluator) = compiler
            .compile_program(&parsed, compile_options())
            .await
            .unwrap();
        let ty = program.result_type().clone();
        let (value, ctx) = evaluator
            .run_with_context(program, Default::default())
            .await
            .unwrap();
        let inner = io_result_type_arg(&ty).unwrap();
        let value = run_io_handle(ctx, value).await.unwrap();
        assert_eq!(
            inner,
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::String),
                Type::builtin(BuiltinTypeId::Bool),
            ])
        );
        let Value::Tuple(xs) = value.value().unwrap() else {
            panic!("expected tuple");
        };
        assert_eq!(xs[0].to_rust::<String>().unwrap(), "hello");
        assert!(xs[1].to_rust::<bool>().unwrap());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cli_io_deep_bind_chain_does_not_recurse_rust_stack() {
        let code = r#"
            import std.io;

            fn go n: i32 -> io.IO i32 =
              if n <= 0 then pure n
              else bind (\_ -> go (n - 1)) (pure n);

            go 10000
        "#;

        let mut builder = Builder::with_prelude(()).unwrap();

        inject_cli_prelude_builder(&mut builder).unwrap();
        let compiler = builder.build_compiler();
        let parsed = parse_rex(code).unwrap();
        let (program, evaluator) = compiler
            .compile_program(&parsed, compile_options())
            .await
            .unwrap();
        let ty = program.result_type().clone();
        let (value, ctx) = evaluator
            .run_with_context(program, Default::default())
            .await
            .unwrap();
        let inner = io_result_type_arg(&ty).unwrap();
        let value = run_io_handle(ctx, value).await.unwrap();
        assert_eq!(inner, Type::builtin(BuiltinTypeId::I32));
        assert_eq!(value.to_rust::<i32>().unwrap(), 0);
    }

    #[tokio::test]
    async fn cli_subprocess_captures_stdout_and_exit_code() {
        let code = r#"
            import std.process;

            let p = process.spawn (process.SpawnOptions {
                cmd = "sh",
                args = ["-c", "printf hi"]
            }) in
              (process.wait p, process.stdout p, process.stderr p)
        "#;

        let mut builder = Builder::with_prelude(()).unwrap();

        inject_cli_prelude_builder(&mut builder).unwrap();
        let compiler = builder.build_compiler();
        let parsed = parse_rex(code).unwrap();
        let (program, evaluator) = compiler
            .compile_program(&parsed, compile_options())
            .await
            .unwrap();
        let ty = program.result_type().clone();
        let value = evaluator.run(program, Default::default()).await.unwrap();
        assert_eq!(
            ty,
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::I32),
                Type::list(Type::builtin(BuiltinTypeId::U8)),
                Type::list(Type::builtin(BuiltinTypeId::U8)),
            ])
        );
        let Value::Tuple(xs) = value.value().unwrap() else {
            panic!("expected tuple");
        };
        assert_eq!(xs[0].to_rust::<i32>().unwrap(), 0);

        let out = xs[1].as_list().unwrap();
        let got: Vec<u8> = out.iter().map(|v| v.to_rust::<u8>().unwrap()).collect();
        assert_eq!(got, b"hi");

        let err = xs[2].as_list().unwrap();
        assert!(err.is_empty());
    }
}
