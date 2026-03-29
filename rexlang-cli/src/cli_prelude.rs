use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use rexlang::{
    BuiltinTypeId, Engine, EngineError, FromPointer, Heap, Library, Pointer, Scheme, Symbol, Type,
    Value, sym, virtual_export_name,
};
use uuid::Uuid;

fn lock_mutex<'a, T>(
    m: &'a Mutex<T>,
    context: &str,
) -> Result<std::sync::MutexGuard<'a, T>, EngineError> {
    m.lock()
        .map_err(|_| EngineError::Internal(format!("{context}: mutex poisoned (this is a bug)")))
}

fn lock_arc_mutex<'a, T>(
    m: &'a Arc<Mutex<T>>,
    context: &str,
) -> Result<std::sync::MutexGuard<'a, T>, EngineError> {
    m.lock()
        .map_err(|_| EngineError::Internal(format!("{context}: mutex poisoned (this is a bug)")))
}

fn unit_pointer(heap: &Heap) -> Result<Pointer, EngineError> {
    heap.alloc_tuple(vec![])
}

#[cfg(test)]
fn value_type_name(value: &Value) -> &'static str {
    value.value_type_name()
}

fn unit_type() -> Type {
    Type::tuple(vec![])
}

fn array_type(elem: Type) -> Type {
    Type::app(Type::builtin(BuiltinTypeId::Array), elem)
}

fn list_to_vec(heap: &Heap, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = *pointer;
    loop {
        let value = heap.get(&cursor)?;
        match value.as_ref() {
            Value::Adt(tag, args) if tag.as_ref() == "Empty" && args.is_empty() => return Ok(out),
            Value::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
                out.push(args[0]);
                cursor = args[1];
            }
            _ => {
                return Err(EngineError::NativeType {
                    expected: "List".into(),
                    got: heap.type_name(&cursor)?.into(),
                });
            }
        }
    }
}

fn array_u8_to_bytes(heap: &Heap, pointer: &Pointer) -> Result<Vec<u8>, EngineError> {
    let elems = heap.pointer_as_array(pointer)?;
    let mut out = Vec::with_capacity(elems.len());
    for elem in &elems {
        out.push(heap.pointer_as_u8(elem)?);
    }
    Ok(out)
}

fn bytes_to_array_u8(heap: &Heap, bytes: Vec<u8>) -> Result<Pointer, EngineError> {
    let out = bytes
        .into_iter()
        .map(|b| heap.alloc_u8(b))
        .collect::<Result<Vec<_>, _>>()?;
    heap.alloc_array(out)
}

#[derive(Default)]
struct SubprocessRegistry {
    procs: Mutex<HashMap<Uuid, Arc<SubprocessEntry>>>,
}

struct SubprocessEntry {
    exit_code: Mutex<Option<i32>>,
    child: Mutex<Option<std::process::Child>>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_done: AtomicBool,
    stderr_done: AtomicBool,
    stdout_thread: Mutex<Option<JoinHandle<io::Result<()>>>>,
    stderr_thread: Mutex<Option<JoinHandle<io::Result<()>>>>,
}

impl SubprocessEntry {
    fn new(child: std::process::Child) -> Self {
        Self {
            exit_code: Mutex::new(None),
            child: Mutex::new(Some(child)),
            stdout: Arc::new(Mutex::new(Vec::new())),
            stderr: Arc::new(Mutex::new(Vec::new())),
            stdout_done: AtomicBool::new(false),
            stderr_done: AtomicBool::new(false),
            stdout_thread: Mutex::new(None),
            stderr_thread: Mutex::new(None),
        }
    }
}

static SUBPROCESSES: OnceLock<SubprocessRegistry> = OnceLock::new();

fn subprocess_registry() -> &'static SubprocessRegistry {
    SUBPROCESSES.get_or_init(SubprocessRegistry::default)
}

pub fn inject_cli_prelude_engine(engine: &mut Engine) -> Result<(), EngineError> {
    engine.inject_tracing_log_function(&virtual_export_name("std.io", "debug"), |s| {
        tracing::debug!("{s}")
    })?;
    engine.inject_tracing_log_function(&virtual_export_name("std.io", "info"), |s| {
        tracing::info!("{s}")
    })?;
    engine.inject_tracing_log_function(&virtual_export_name("std.io", "warn"), |s| {
        tracing::warn!("{s}")
    })?;
    engine.inject_tracing_log_function(&virtual_export_name("std.io", "error"), |s| {
        tracing::error!("{s}")
    })?;

    inject_cli_io_natives(engine)?;
    inject_cli_process_natives(engine)?;
    Ok(())
}

fn inject_cli_io_natives(engine: &mut Engine) -> Result<(), EngineError> {
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let u8_ty = Type::builtin(BuiltinTypeId::U8);
    let array_u8 = array_type(u8_ty);
    let mut library = Library::new("std.io");

    let read_all_sym = sym("read_all");
    library.export_native_async(
        "read_all",
        Scheme::new(vec![], vec![], Type::fun(i32_ty.clone(), array_u8.clone())),
        1,
        move |engine, _, args| {
            let read_all_sym = read_all_sym.clone();
            Box::pin(async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: read_all_sym,
                        expected: 1,
                        got: args.len(),
                    });
                }
                let fd = i32::from_pointer(&engine.heap, &args[0])?;

                if fd != 0 {
                    return Err(EngineError::Internal(format!(
                        "read_all only supports fd 0 (stdin), got {fd}"
                    )));
                }

                let mut buf = Vec::new();
                io::stdin()
                    .read_to_end(&mut buf)
                    .map_err(|e| EngineError::Internal(format!("read_all failed: {e}")))?;
                bytes_to_array_u8(&engine.heap, buf)
            })
        },
    )?;

    let write_all_sym = sym("write_all");
    library.export_native_async(
        "write_all",
        Scheme::new(
            vec![],
            vec![],
            Type::fun(i32_ty, Type::fun(array_u8, unit_type())),
        ),
        2,
        move |engine, _, args| {
            let write_all_sym = write_all_sym.clone();
            Box::pin(async move {
                if args.len() != 2 {
                    return Err(EngineError::NativeArity {
                        name: write_all_sym,
                        expected: 2,
                        got: args.len(),
                    });
                }
                let fd = i32::from_pointer(&engine.heap, &args[0])?;
                let bytes = array_u8_to_bytes(&engine.heap, &args[1])?;

                match fd {
                    1 => {
                        let mut out = io::stdout().lock();
                        out.write_all(&bytes)
                            .and_then(|()| out.flush())
                            .map_err(|e| EngineError::Internal(format!("write_all failed: {e}")))?;
                    }
                    2 => {
                        let mut out = io::stderr().lock();
                        out.write_all(&bytes)
                            .and_then(|()| out.flush())
                            .map_err(|e| EngineError::Internal(format!("write_all failed: {e}")))?;
                    }
                    _ => {
                        return Err(EngineError::Internal(format!(
                            "write_all only supports fd 1 (stdout) and 2 (stderr), got {fd}"
                        )));
                    }
                }

                unit_pointer(&engine.heap)
            })
        },
    )?;

    engine.inject_library(library)
}

fn inject_cli_process_natives(engine: &mut Engine) -> Result<(), EngineError> {
    let subprocess_name = virtual_export_name("std.process", "Subprocess");
    let subprocess_ctor = sym(&subprocess_name);
    let subprocess = Type::con(&subprocess_name, 0);
    let string = Type::builtin(BuiltinTypeId::String);
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let list_string = Type::app(Type::builtin(BuiltinTypeId::List), string.clone());
    let mut library = Library::new("std.process");
    let opts = Type::record(vec![
        (sym("cmd"), string.clone()),
        (sym("args"), list_string),
    ]);

    let spawn_sym = sym("spawn");
    let subprocess_ctor_for_spawn = subprocess_ctor.clone();
    library.export_native_async(
        "spawn",
        Scheme::new(vec![], vec![], Type::fun(opts, subprocess.clone())),
        1,
        move |engine, _, args| {
            let spawn_sym = spawn_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_spawn.clone();
            Box::pin(async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: spawn_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let map = engine.heap.pointer_as_dict(&args[0])?;

                let cmd_pointer = map
                    .get(&sym("cmd"))
                    .cloned()
                    .ok_or_else(|| EngineError::Internal("spawn missing `cmd`".into()))?;
                let cmd = String::from_pointer(&engine.heap, &cmd_pointer)?;

                let args_pointer = map
                    .get(&sym("args"))
                    .cloned()
                    .ok_or_else(|| EngineError::Internal("spawn missing `args`".into()))?;
                let args_list = list_to_vec(&engine.heap, &args_pointer)?;
                let mut args_vec = Vec::with_capacity(args_list.len());
                for arg in args_list {
                    args_vec.push(String::from_pointer(&engine.heap, &arg)?);
                }

                let mut child = Command::new(cmd)
                    .args(args_vec)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| EngineError::Internal(format!("spawn failed: {e}")))?;

                let mut stdout = child.stdout.take().ok_or_else(|| {
                    EngineError::Internal("spawn failed to capture stdout".into())
                })?;
                let mut stderr = child.stderr.take().ok_or_else(|| {
                    EngineError::Internal("spawn failed to capture stderr".into())
                })?;

                let id = Uuid::new_v4();
                let entry = Arc::new(SubprocessEntry::new(child));

                {
                    let stdout_buf = entry.stdout.clone();
                    let entry_for_done = entry.clone();
                    let handle = thread::spawn(move || {
                        let mut tmp = [0u8; 8192];
                        loop {
                            let n = stdout.read(&mut tmp)?;
                            if n == 0 {
                                break;
                            }
                            if let Ok(mut buf) = stdout_buf.lock() {
                                buf.extend_from_slice(&tmp[..n]);
                            }
                        }
                        entry_for_done.stdout_done.store(true, Ordering::Release);
                        Ok(())
                    });
                    *lock_mutex(&entry.stdout_thread, "std.process.spawn stdout_thread")? =
                        Some(handle);
                }

                {
                    let stderr_buf = entry.stderr.clone();
                    let entry_for_done = entry.clone();
                    let handle = thread::spawn(move || {
                        let mut tmp = [0u8; 8192];
                        loop {
                            let n = stderr.read(&mut tmp)?;
                            if n == 0 {
                                break;
                            }
                            if let Ok(mut buf) = stderr_buf.lock() {
                                buf.extend_from_slice(&tmp[..n]);
                            }
                        }
                        entry_for_done.stderr_done.store(true, Ordering::Release);
                        Ok(())
                    });
                    *lock_mutex(&entry.stderr_thread, "std.process.spawn stderr_thread")? =
                        Some(handle);
                }

                subprocess_registry()
                    .procs
                    .lock()
                    .map_err(|_| {
                        EngineError::Internal(
                            "std.process.spawn: subprocess registry mutex poisoned (this is a bug)"
                                .into(),
                        )
                    })?
                    .insert(id, entry);

                let mut payload = BTreeMap::new();
                payload.insert(sym("id"), engine.heap.alloc_uuid(id)?);
                let payload = engine.heap.alloc_dict(payload)?;
                engine.heap.alloc_adt(subprocess_ctor, vec![payload])
            })
        },
    )?;

    let wait_sym = sym("wait");
    let subprocess_ctor_for_wait = subprocess_ctor.clone();
    library.export_native_async(
        "wait",
        Scheme::new(vec![], vec![], Type::fun(subprocess.clone(), i32_ty)),
        1,
        move |engine, _, args| {
            let wait_sym = wait_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_wait.clone();
            Box::pin(async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: wait_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let id = subprocess_id(&engine.heap, &args[0], &subprocess_ctor)?;
                let entry = subprocess_get(&id, wait_sym.as_ref())?;

                if let Some(code) = *lock_mutex(&entry.exit_code, "std.process.wait exit_code")? {
                    return engine.heap.alloc_i32(code);
                }

                let status = {
                    let mut child_guard = lock_mutex(&entry.child, "std.process.wait child")?;
                    let Some(child) = child_guard.as_mut() else {
                        return Err(EngineError::Internal("subprocess already reaped".into()));
                    };
                    child
                        .wait()
                        .map_err(|e| EngineError::Internal(format!("wait failed: {e}")))?
                };

                let code = status.code().unwrap_or(-1);
                *lock_mutex(&entry.exit_code, "std.process.wait exit_code")? = Some(code);

                // Ensure pipes are drained.
                if let Some(handle) = entry
                    .stdout_thread
                    .lock()
                    .map_err(|_| {
                        EngineError::Internal(
                            "std.process.wait: stdout_thread mutex poisoned (this is a bug)".into(),
                        )
                    })?
                    .take()
                {
                    let _ = handle.join();
                }
                if let Some(handle) = entry
                    .stderr_thread
                    .lock()
                    .map_err(|_| {
                        EngineError::Internal(
                            "std.process.wait: stderr_thread mutex poisoned (this is a bug)".into(),
                        )
                    })?
                    .take()
                {
                    let _ = handle.join();
                }

                engine.heap.alloc_i32(code)
            })
        },
    )?;

    let stdout_sym = sym("stdout");
    let subprocess_ctor_for_stdout = subprocess_ctor.clone();
    library.export_native_async(
        "stdout",
        Scheme::new(
            vec![],
            vec![],
            Type::fun(
                subprocess.clone(),
                array_type(Type::builtin(BuiltinTypeId::U8)),
            ),
        ),
        1,
        move |engine, _, args| {
            let stdout_sym = stdout_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_stdout.clone();
            Box::pin(async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: stdout_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let id = subprocess_id(&engine.heap, &args[0], &subprocess_ctor)?;
                let entry = subprocess_get(&id, stdout_sym.as_ref())?;
                let bytes = lock_arc_mutex(&entry.stdout, "std.process.stdout buffer")?.clone();
                bytes_to_array_u8(&engine.heap, bytes)
            })
        },
    )?;

    let stderr_sym = sym("stderr");
    let subprocess_ctor_for_stderr = subprocess_ctor.clone();
    library.export_native_async(
        "stderr",
        Scheme::new(
            vec![],
            vec![],
            Type::fun(subprocess, array_type(Type::builtin(BuiltinTypeId::U8))),
        ),
        1,
        move |engine, _, args| {
            let stderr_sym = stderr_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_stderr.clone();
            Box::pin(async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: stderr_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let id = subprocess_id(&engine.heap, &args[0], &subprocess_ctor)?;
                let entry = subprocess_get(&id, stderr_sym.as_ref())?;
                let bytes = lock_arc_mutex(&entry.stderr, "std.process.stderr buffer")?.clone();
                bytes_to_array_u8(&engine.heap, bytes)
            })
        },
    )?;

    engine.inject_library(library)
}

fn subprocess_id(heap: &Heap, pointer: &Pointer, tag: &Symbol) -> Result<Uuid, EngineError> {
    let (got_tag, args) = heap.pointer_as_adt(pointer)?;
    if &got_tag != tag || args.len() != 1 {
        return Err(EngineError::NativeType {
            expected: "Subprocess".into(),
            got: heap.type_name(pointer)?.into(),
        });
    }
    let map = heap.pointer_as_dict(&args[0])?;
    let id_pointer = map
        .get(&sym("id"))
        .cloned()
        .ok_or_else(|| EngineError::Internal("Subprocess missing id".into()))?;
    heap.pointer_as_uuid(&id_pointer)
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

#[cfg(test)]
mod tests {
    use rexlang::{Engine, GasMeter, assert_pointer_eq};

    use super::*;

    fn unlimited_gas() -> GasMeter {
        GasMeter::default()
    }

    #[tokio::test]
    async fn cli_prelude_typecheck_smoke() {
        let code = r#"
            import std.process
            import std.io

            let p = process.spawn { cmd = "sh", args = ["-c", "printf hi"] } in
              io.write_all 1 (process.stdout p)
        "#;

        let mut engine = Engine::with_prelude(()).unwrap();
        engine.add_default_resolvers();
        inject_cli_prelude_engine(&mut engine).unwrap();
        let mut gas = unlimited_gas();
        engine.eval_snippet(code, &mut gas).await.unwrap();
    }

    #[tokio::test]
    async fn cli_subprocess_captures_stdout_and_exit_code() {
        let code = r#"
            import std.process

            let p = process.spawn { cmd = "sh", args = ["-c", "printf hi"] } in
              (process.wait p, process.stdout p, process.stderr p)
        "#;

        let mut engine = Engine::with_prelude(()).unwrap();
        engine.add_default_resolvers();
        inject_cli_prelude_engine(&mut engine).unwrap();
        let mut gas = unlimited_gas();
        let (value, ty) = engine.eval_snippet(code, &mut gas).await.unwrap();
        assert_eq!(
            ty,
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::I32),
                array_type(Type::builtin(BuiltinTypeId::U8)),
                array_type(Type::builtin(BuiltinTypeId::U8)),
            ])
        );
        let value = engine
            .heap
            .get(&value)
            .map(|value| value.as_ref().clone())
            .unwrap();
        let Value::Tuple(xs) = value else {
            panic!("expected tuple");
        };
        assert_pointer_eq!(
            &engine.heap,
            xs[0].clone(),
            engine.heap.alloc_i32(0).unwrap()
        );

        let Value::Array(out) = engine
            .heap
            .get(&xs[1])
            .map(|value| value.as_ref().clone())
            .unwrap()
        else {
            panic!("expected stdout bytes");
        };
        let got: Vec<u8> = out
            .iter()
            .map(|v| {
                match engine
                    .heap
                    .get(v)
                    .map(|value| value.as_ref().clone())
                    .unwrap()
                {
                    Value::U8(b) => b,
                    other => panic!("expected u8, got {}", value_type_name(&other)),
                }
            })
            .collect();
        assert_eq!(got, b"hi");

        let Value::Array(err) = engine
            .heap
            .get(&xs[2])
            .map(|value| value.as_ref().clone())
            .unwrap()
        else {
            panic!("expected stderr bytes");
        };
        assert!(err.is_empty());
    }
}
