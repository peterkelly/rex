use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use rex_ast::expr::{Symbol, sym};
use rex_engine::{Engine, EngineError, Heap, Value, virtual_export_name};
use rex_ts::{Scheme, Type};
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

fn unit_value(heap: &Heap) -> Value {
    heap.alloc_tuple(vec![])
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Bool(..) => "bool",
        Value::U8(..) => "u8",
        Value::U16(..) => "u16",
        Value::U32(..) => "u32",
        Value::U64(..) => "u64",
        Value::I8(..) => "i8",
        Value::I16(..) => "i16",
        Value::I32(..) => "i32",
        Value::I64(..) => "i64",
        Value::F32(..) => "f32",
        Value::F64(..) => "f64",
        Value::String(..) => "string",
        Value::Uuid(..) => "uuid",
        Value::DateTime(..) => "datetime",
        Value::Tuple(..) => "tuple",
        Value::Array(..) => "array",
        Value::Dict(..) => "dict",
        Value::Adt(tag, ..) if tag.as_ref() == "Empty" || tag.as_ref() == "Cons" => "list",
        Value::Adt(..) => "adt",
        Value::Closure(..) => "closure",
        Value::Native(..) => "native",
        Value::Overloaded(..) => "overloaded",
    }
}

fn unit_type() -> Type {
    Type::tuple(vec![])
}

fn array_type(elem: Type) -> Type {
    Type::app(Type::con("Array", 1), elem)
}

fn list_to_vec(value: &Value, name: &str) -> Result<Vec<Value>, EngineError> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if tag.as_ref() == "Empty" && args.is_empty() => return Ok(out),
            Value::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
                out.push(args[0].clone());
                cur = &args[1];
            }
            other => {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "List".into(),
                    got: value_type_name(other).into(),
                });
            }
        }
    }
}

fn array_u8_to_bytes(value: &Value, name: &str) -> Result<Vec<u8>, EngineError> {
    let Value::Array(elems) = value else {
        return Err(EngineError::NativeType {
            name: sym(name),
            expected: "Array u8".into(),
            got: value_type_name(value).into(),
        });
    };
    let mut out = Vec::with_capacity(elems.len());
    for elem in elems {
        match elem {
            Value::U8(b) => out.push(*b),
            other => {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "u8".into(),
                    got: value_type_name(other).into(),
                });
            }
        }
    }
    Ok(out)
}

fn bytes_to_array_u8(heap: &Heap, bytes: Vec<u8>) -> Value {
    let out = bytes.into_iter().map(|b| heap.alloc_u8(b)).collect();
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
    let i32_ty = Type::con("i32", 0);
    let u8_ty = Type::con("u8", 0);
    let array_u8 = array_type(u8_ty);

    let read_all_name = virtual_export_name("std.io", "read_all");
    let read_all_sym = sym(&read_all_name);
    engine.inject_native_scheme_typed_async(
        &read_all_name,
        Scheme::new(vec![], vec![], Type::fun(i32_ty.clone(), array_u8.clone())),
        1,
        move |engine, _call_type, args| {
            let read_all_sym = read_all_sym.clone();
            async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: read_all_sym,
                        expected: 1,
                        got: args.len(),
                    });
                }
                let Value::I32(fd) = args[0] else {
                    return Err(EngineError::NativeType {
                        name: read_all_sym,
                        expected: "i32".into(),
                        got: value_type_name(&args[0]).into(),
                    });
                };

                if fd != 0 {
                    return Err(EngineError::Internal(format!(
                        "read_all only supports fd 0 (stdin), got {fd}"
                    )));
                }

                let mut buf = Vec::new();
                io::stdin()
                    .read_to_end(&mut buf)
                    .map_err(|e| EngineError::Internal(format!("read_all failed: {e}")))?;
                Ok(bytes_to_array_u8(engine.heap(), buf))
            }
        },
    )?;

    let write_all_name = virtual_export_name("std.io", "write_all");
    let write_all_sym = sym(&write_all_name);
    engine.inject_native_scheme_typed_async(
        &write_all_name,
        Scheme::new(
            vec![],
            vec![],
            Type::fun(i32_ty, Type::fun(array_u8, unit_type())),
        ),
        2,
        move |engine, _call_type, args| {
            let write_all_sym = write_all_sym.clone();
            async move {
                if args.len() != 2 {
                    return Err(EngineError::NativeArity {
                        name: write_all_sym,
                        expected: 2,
                        got: args.len(),
                    });
                }
                let Value::I32(fd) = args[0] else {
                    return Err(EngineError::NativeType {
                        name: write_all_sym,
                        expected: "i32".into(),
                        got: value_type_name(&args[0]).into(),
                    });
                };
                let bytes = array_u8_to_bytes(&args[1], write_all_sym.as_ref())?;

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

                Ok(unit_value(engine.heap()))
            }
        },
    )?;

    Ok(())
}

fn inject_cli_process_natives(engine: &mut Engine) -> Result<(), EngineError> {
    let subprocess_name = virtual_export_name("std.process", "Subprocess");
    let subprocess_ctor = sym(&subprocess_name);
    let subprocess = Type::con(&subprocess_name, 0);
    let string = Type::con("string", 0);
    let i32_ty = Type::con("i32", 0);
    let list_string = Type::app(Type::con("List", 1), string.clone());
    let opts = Type::record(vec![
        (sym("cmd"), string.clone()),
        (sym("args"), list_string),
    ]);

    let spawn_name = virtual_export_name("std.process", "spawn");
    let spawn_sym = sym(&spawn_name);
    let subprocess_ctor_for_spawn = subprocess_ctor.clone();
    engine.inject_native_scheme_typed_async(
        &spawn_name,
        Scheme::new(vec![], vec![], Type::fun(opts, subprocess.clone())),
        1,
        move |engine, _call_type, args| {
            let spawn_sym = spawn_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_spawn.clone();
            async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: spawn_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let Value::Dict(map) = &args[0] else {
                    return Err(EngineError::NativeType {
                        name: spawn_sym.clone(),
                        expected: "{ cmd: string, args: List string }".into(),
                        got: value_type_name(&args[0]).into(),
                    });
                };

                let Value::String(cmd) = map
                    .get(&sym("cmd"))
                    .ok_or_else(|| EngineError::Internal("spawn missing `cmd`".into()))?
                else {
                    return Err(EngineError::NativeType {
                        name: spawn_sym.clone(),
                        expected: "string".into(),
                        got: map
                            .get(&sym("cmd"))
                            .map(value_type_name)
                            .unwrap_or("unknown")
                            .into(),
                    });
                };

                let args_value = map
                    .get(&sym("args"))
                    .ok_or_else(|| EngineError::Internal("spawn missing `args`".into()))?;
                let args_list = list_to_vec(args_value, spawn_sym.as_ref())?;
                let mut args_vec = Vec::with_capacity(args_list.len());
                for arg in args_list {
                    match arg {
                        Value::String(s) => args_vec.push(s),
                        other => {
                            return Err(EngineError::NativeType {
                                name: spawn_sym.clone(),
                                expected: "string".into(),
                                got: value_type_name(&other).into(),
                            });
                        }
                    }
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
                payload.insert(sym("id"), engine.heap().alloc_uuid(id));
                Ok(engine
                    .heap()
                    .alloc_adt(subprocess_ctor, vec![engine.heap().alloc_dict(payload)]))
            }
        },
    )?;

    let wait_name = virtual_export_name("std.process", "wait");
    let wait_sym = sym(&wait_name);
    let subprocess_ctor_for_wait = subprocess_ctor.clone();
    engine.inject_native_scheme_typed_async(
        &wait_name,
        Scheme::new(vec![], vec![], Type::fun(subprocess.clone(), i32_ty)),
        1,
        move |engine, _call_type, args| {
            let wait_sym = wait_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_wait.clone();
            async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: wait_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let id = subprocess_id(&args[0], &subprocess_ctor, wait_sym.as_ref())?;
                let entry = subprocess_get(&id, wait_sym.as_ref())?;

                if let Some(code) = *lock_mutex(&entry.exit_code, "std.process.wait exit_code")? {
                    return Ok(engine.heap().alloc_i32(code));
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

                Ok(engine.heap().alloc_i32(code))
            }
        },
    )?;

    let stdout_name = virtual_export_name("std.process", "stdout");
    let stdout_sym = sym(&stdout_name);
    let subprocess_ctor_for_stdout = subprocess_ctor.clone();
    engine.inject_native_scheme_typed_async(
        &stdout_name,
        Scheme::new(
            vec![],
            vec![],
            Type::fun(subprocess.clone(), array_type(Type::con("u8", 0))),
        ),
        1,
        move |engine, _call_type, args| {
            let stdout_sym = stdout_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_stdout.clone();
            async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: stdout_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let id = subprocess_id(&args[0], &subprocess_ctor, stdout_sym.as_ref())?;
                let entry = subprocess_get(&id, stdout_sym.as_ref())?;
                let bytes = lock_arc_mutex(&entry.stdout, "std.process.stdout buffer")?.clone();
                Ok(bytes_to_array_u8(engine.heap(), bytes))
            }
        },
    )?;

    let stderr_name = virtual_export_name("std.process", "stderr");
    let stderr_sym = sym(&stderr_name);
    let subprocess_ctor_for_stderr = subprocess_ctor.clone();
    engine.inject_native_scheme_typed_async(
        &stderr_name,
        Scheme::new(
            vec![],
            vec![],
            Type::fun(subprocess, array_type(Type::con("u8", 0))),
        ),
        1,
        move |engine, _call_type, args| {
            let stderr_sym = stderr_sym.clone();
            let subprocess_ctor = subprocess_ctor_for_stderr.clone();
            async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: stderr_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let id = subprocess_id(&args[0], &subprocess_ctor, stderr_sym.as_ref())?;
                let entry = subprocess_get(&id, stderr_sym.as_ref())?;
                let bytes = lock_arc_mutex(&entry.stderr, "std.process.stderr buffer")?.clone();
                Ok(bytes_to_array_u8(engine.heap(), bytes))
            }
        },
    )?;

    Ok(())
}

fn subprocess_id(value: &Value, tag: &Symbol, name: &str) -> Result<Uuid, EngineError> {
    match value {
        Value::Adt(got_tag, args) if got_tag == tag && args.len() == 1 => {
            let Value::Dict(map) = &args[0] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "Subprocess".into(),
                    got: value_type_name(value).into(),
                });
            };
            let Value::Uuid(id) = map
                .get(&sym("id"))
                .ok_or_else(|| EngineError::Internal("Subprocess missing id".into()))?
            else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "uuid".into(),
                    got: map
                        .get(&sym("id"))
                        .map(value_type_name)
                        .unwrap_or("unknown")
                        .into(),
                });
            };
            Ok(*id)
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Subprocess".into(),
            got: value_type_name(value).into(),
        }),
    }
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
    use std::thread;

    use rex_engine::Engine;

    use super::*;

    #[test]
    fn cli_prelude_typecheck_smoke() {
        let code = r#"
            import std.process
            import std.io

            let p = process.spawn { cmd = "sh", args = ["-c", "printf hi"] } in
              io.write_all 1 (process.stdout p)
        "#;

        let handle = thread::Builder::new()
            .name("cli-prelude-typecheck".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let mut engine = Engine::with_prelude().unwrap();
                inject_cli_prelude_engine(&mut engine).unwrap();
                engine.add_default_resolvers();
                engine.eval_snippet(code).unwrap();
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn cli_subprocess_captures_stdout_and_exit_code() {
        let code = r#"
            import std.process

            let p = process.spawn { cmd = "sh", args = ["-c", "printf hi"] } in
              (process.wait p, process.stdout p, process.stderr p)
        "#;

        let handle = thread::Builder::new()
            .name("cli-subprocess-eval".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let mut engine = Engine::with_prelude().unwrap();
                inject_cli_prelude_engine(&mut engine).unwrap();
                engine.add_default_resolvers();
                let value = engine.eval_snippet(code).unwrap();
                let Value::Tuple(xs) = value else {
                    panic!("expected tuple");
                };
                assert_eq!(xs[0], engine.heap().alloc_i32(0));

                let Value::Array(out) = &xs[1] else {
                    panic!("expected stdout bytes");
                };
                let got: Vec<u8> = out
                    .iter()
                    .map(|v| match v {
                        Value::U8(b) => *b,
                        other => panic!("expected u8, got {}", value_type_name(other)),
                    })
                    .collect();
                assert_eq!(got, b"hi");

                let Value::Array(err) = &xs[2] else {
                    panic!("expected stderr bytes");
                };
                assert!(err.is_empty());
            })
            .unwrap();
        handle.join().unwrap();
    }
}
