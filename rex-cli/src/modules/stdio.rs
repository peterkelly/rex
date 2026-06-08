//! CLI-only `std.io` implementation used to exercise Rex host actions.
//!
//! This module exists for `rex-cli` examples and tests, especially tests of the
//! monadic `IO` surface (`pure`, `map`, `ap`, and `bind`) and the host-action
//! runner that executes those values at the CLI boundary. It is intentionally
//! not a general recommendation for embedders. Real hosts commonly run
//! user-supplied Rex code inside a sandboxed application model, and exposing
//! broad filesystem, working-directory, stdin, stdout, and stderr access would
//! usually violate that sandbox. Embedders should normally provide narrower,
//! domain-specific host actions instead.

use futures::FutureExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rex::{
    ast::Symbol,
    engine::{
        Builder, Context, Declarations, EngineError, FromRex, Handle, HostAction, HostActionEffect,
        IntoRex, Module, run_host_action,
    },
    parser::parse as parse_rex,
    typesystem::{AdtDecl, BuiltinTypeId, Scheme, Type, TypeKind, TypeVar, TypeVarSupply},
};
use tokio::fs;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

type IoEffect = HostActionEffect<()>;
type IoAction = HostAction<()>;

#[derive(Default)]
struct IoRegistry {
    actions: Mutex<HashMap<Uuid, IoAction>>,
}

static IO_ACTIONS: OnceLock<IoRegistry> = OnceLock::new();

fn io_registry() -> &'static IoRegistry {
    IO_ACTIONS.get_or_init(IoRegistry::default)
}

pub(crate) fn inject_cli_io_natives(builder: &mut Builder) -> Result<(), EngineError> {
    let mut module = Module::new("std.io");
    module.add_adt_decl(io_adt_decl())?;
    let io_decls = Declarations::from(io_typeclass_decls()?);
    if !io_decls.types.is_empty()
        || !io_decls.fns.is_empty()
        || !io_decls.declare_fns.is_empty()
        || !io_decls.imports.is_empty()
        || !io_decls.classes.is_empty()
    {
        return Err(EngineError::Internal(
            "std.io typeclass declarations unexpectedly included non-instance declarations".into(),
        ));
    }
    for instance in io_decls.instances {
        module.add_instance(instance);
    }

    module.export_native("io_pure", io_pure_scheme(), 1, |ctx, _typ, args| {
        let value = args
            .first()
            .cloned()
            .ok_or_else(|| EngineError::Internal("std.io.io_pure missing value".into()))?;
        alloc_io_action(&ctx, IoAction::Pure(value))
    })?;
    module.export_native("io_map", io_map_scheme(), 2, |ctx, typ, args| {
        let (arg_tys, _ret_ty) = split_fun_chain(typ, 2)?;
        let f_type = arg_tys[0].clone();
        let action = args
            .get(1)
            .cloned()
            .ok_or_else(|| EngineError::Internal("std.io.io_map missing action".into()))?;
        let input_type = io_type_arg(&arg_tys[1]).ok_or_else(|| {
            EngineError::Internal(format!(
                "std.io.io_map expected IO input, got {}",
                arg_tys[1]
            ))
        })?;
        let f = args
            .first()
            .cloned()
            .ok_or_else(|| EngineError::Internal("std.io.io_map missing callback".into()))?;
        alloc_io_action(
            &ctx,
            IoAction::Map {
                f,
                f_type,
                input_type,
                action,
            },
        )
    })?;
    module.export_native("io_ap", io_ap_scheme(), 2, |ctx, typ, args| {
        let (arg_tys, _ret_ty) = split_fun_chain(typ, 2)?;
        let f_type = io_type_arg(&arg_tys[0]).ok_or_else(|| {
            EngineError::Internal(format!(
                "std.io.io_ap expected IO function, got {}",
                arg_tys[0]
            ))
        })?;
        let input_type = io_type_arg(&arg_tys[1]).ok_or_else(|| {
            EngineError::Internal(format!(
                "std.io.io_ap expected IO input, got {}",
                arg_tys[1]
            ))
        })?;
        let f_action = args
            .first()
            .cloned()
            .ok_or_else(|| EngineError::Internal("std.io.io_ap missing function action".into()))?;
        let action = args
            .get(1)
            .cloned()
            .ok_or_else(|| EngineError::Internal("std.io.io_ap missing value action".into()))?;
        alloc_io_action(
            &ctx,
            IoAction::Ap {
                f_action,
                action,
                f_type,
                input_type,
            },
        )
    })?;
    module.export_native("io_bind", io_bind_scheme(), 2, |ctx, typ, args| {
        let (arg_tys, _ret_ty) = split_fun_chain(typ, 2)?;
        let f_type = arg_tys[0].clone();
        let input_type = io_type_arg(&arg_tys[1]).ok_or_else(|| {
            EngineError::Internal(format!(
                "std.io.io_bind expected IO input, got {}",
                arg_tys[1]
            ))
        })?;
        let f = args
            .first()
            .cloned()
            .ok_or_else(|| EngineError::Internal("std.io.io_bind missing callback".into()))?;
        let action = args
            .get(1)
            .cloned()
            .ok_or_else(|| EngineError::Internal("std.io.io_bind missing action".into()))?;
        alloc_io_action(
            &ctx,
            IoAction::Bind {
                f,
                f_type,
                input_type,
                action,
            },
        )
    })?;

    export_io0(&mut module, "read_stdin", io_of(string_type()), |_ctx| {
        Arc::new(|ctx| {
            async move {
                let mut input = String::new();
                io::stdin()
                    .read_to_string(&mut input)
                    .await
                    .map_err(|e| EngineError::Internal(format!("std.io.read_stdin failed: {e}")))?;
                input.into_rex(ctx.heap())
            }
            .boxed()
        })
    })?;
    export_io1(
        &mut module,
        "write_stdout",
        string_type(),
        unit_type(),
        |message: String| {
            Arc::new(move |ctx| {
                let message = message.clone();
                async move {
                    let mut out = io::stdout();
                    out.write_all(message.as_bytes()).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.write_stdout failed: {e}"))
                    })?;
                    out.flush().await.map_err(|e| {
                        EngineError::Internal(format!("std.io.write_stdout failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "write_stderr",
        string_type(),
        unit_type(),
        |message: String| {
            Arc::new(move |ctx| {
                let message = message.clone();
                async move {
                    let mut out = io::stderr();
                    out.write_all(message.as_bytes()).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.write_stderr failed: {e}"))
                    })?;
                    out.flush().await.map_err(|e| {
                        EngineError::Internal(format!("std.io.write_stderr failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "read_file",
        string_type(),
        string_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    let contents = fs::read_to_string(&path).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.read_file `{path}` failed: {e}"))
                    })?;
                    contents.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io2(
        &mut module,
        "write_file",
        string_type(),
        string_type(),
        unit_type(),
        |path: String, contents: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                let contents = contents.clone();
                async move {
                    fs::write(&path, contents).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.write_file `{path}` failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io2(
        &mut module,
        "append_file",
        string_type(),
        string_type(),
        unit_type(),
        |path: String, contents: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                let contents = contents.clone();
                async move {
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .await
                        .map_err(|e| {
                            EngineError::Internal(format!(
                                "std.io.append_file `{path}` failed: {e}"
                            ))
                        })?;
                    file.write_all(contents.as_bytes()).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.append_file `{path}` failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "remove_file",
        string_type(),
        unit_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    fs::remove_file(&path).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.remove_file `{path}` failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io2(
        &mut module,
        "copy_file",
        string_type(),
        string_type(),
        i64_type(),
        |from: String, to: String| {
            Arc::new(move |ctx| {
                let from = from.clone();
                let to = to.clone();
                async move {
                    let bytes = fs::copy(&from, &to).await.map_err(|e| {
                        EngineError::Internal(format!(
                            "std.io.copy_file `{from}` to `{to}` failed: {e}"
                        ))
                    })?;
                    let bytes = i64::try_from(bytes).map_err(|_| {
                        EngineError::Internal("std.io.copy_file byte count overflowed i64".into())
                    })?;
                    bytes.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io2(
        &mut module,
        "rename",
        string_type(),
        string_type(),
        unit_type(),
        |from: String, to: String| {
            Arc::new(move |ctx| {
                let from = from.clone();
                let to = to.clone();
                async move {
                    fs::rename(&from, &to).await.map_err(|e| {
                        EngineError::Internal(format!(
                            "std.io.rename `{from}` to `{to}` failed: {e}"
                        ))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "create_dir",
        string_type(),
        unit_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    fs::create_dir(&path).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.create_dir `{path}` failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "create_dir_all",
        string_type(),
        unit_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    fs::create_dir_all(&path).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.create_dir_all `{path}` failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "remove_dir",
        string_type(),
        unit_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    fs::remove_dir(&path).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.remove_dir `{path}` failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "remove_dir_all",
        string_type(),
        unit_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    fs::remove_dir_all(&path).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.remove_dir_all `{path}` failed: {e}"))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "read_dir",
        string_type(),
        array_of(string_type()),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    let mut entries = fs::read_dir(&path).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.read_dir `{path}` failed: {e}"))
                    })?;
                    let mut paths = Vec::new();
                    while let Some(entry) = entries.next_entry().await.map_err(|e| {
                        EngineError::Internal(format!("std.io.read_dir `{path}` failed: {e}"))
                    })? {
                        paths.push(entry.path().display().to_string());
                    }
                    paths.sort();
                    paths.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "exists",
        string_type(),
        bool_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    fs::try_exists(&path)
                        .await
                        .map_err(|e| {
                            EngineError::Internal(format!("std.io.exists `{path}` failed: {e}"))
                        })?
                        .into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "is_file",
        string_type(),
        bool_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    let is_file = fs::metadata(&path)
                        .await
                        .map(|m| m.is_file())
                        .unwrap_or(false);
                    is_file.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "is_dir",
        string_type(),
        bool_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    let is_dir = fs::metadata(&path)
                        .await
                        .map(|m| m.is_dir())
                        .unwrap_or(false);
                    is_dir.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io0(&mut module, "current_dir", io_of(string_type()), |_ctx| {
        Arc::new(|ctx| {
            async move {
                let dir = std::env::current_dir().map_err(|e| {
                    EngineError::Internal(format!("std.io.current_dir failed: {e}"))
                })?;
                dir.display().to_string().into_rex(ctx.heap())
            }
            .boxed()
        })
    })?;
    export_io1(
        &mut module,
        "set_current_dir",
        string_type(),
        unit_type(),
        |path: String| {
            Arc::new(move |ctx| {
                let path = path.clone();
                async move {
                    std::env::set_current_dir(&path).map_err(|e| {
                        EngineError::Internal(format!(
                            "std.io.set_current_dir `{path}` failed: {e}"
                        ))
                    })?;
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "debug",
        string_type(),
        string_type(),
        |message: String| {
            Arc::new(move |ctx| {
                let message = message.clone();
                async move {
                    tracing::debug!("{message}");
                    message.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "info",
        string_type(),
        string_type(),
        |message: String| {
            Arc::new(move |ctx| {
                let message = message.clone();
                async move {
                    tracing::info!("{message}");
                    message.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "warn",
        string_type(),
        string_type(),
        |message: String| {
            Arc::new(move |ctx| {
                let message = message.clone();
                async move {
                    tracing::warn!("{message}");
                    message.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "error",
        string_type(),
        string_type(),
        |message: String| {
            Arc::new(move |ctx| {
                let message = message.clone();
                async move {
                    tracing::error!("{message}");
                    message.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io1(
        &mut module,
        "read_all",
        i32_type(),
        array_of(u8_type()),
        |fd: i32| {
            Arc::new(move |ctx| {
                async move {
                    if fd != 0 {
                        return Err(EngineError::Internal(format!(
                            "std.io.read_all only supports fd 0 (stdin), got {fd}"
                        )));
                    }
                    let mut buf = Vec::new();
                    io::stdin().read_to_end(&mut buf).await.map_err(|e| {
                        EngineError::Internal(format!("std.io.read_all failed: {e}"))
                    })?;
                    buf.into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;
    export_io2(
        &mut module,
        "write_all",
        i32_type(),
        array_of(u8_type()),
        unit_type(),
        |fd: i32, bytes: Vec<u8>| {
            Arc::new(move |ctx| {
                let bytes = bytes.clone();
                async move {
                    match fd {
                        1 => {
                            let mut out = io::stdout();
                            out.write_all(&bytes).await.map_err(|e| {
                                EngineError::Internal(format!("std.io.write_all failed: {e}"))
                            })?;
                            out.flush().await.map_err(|e| {
                                EngineError::Internal(format!("std.io.write_all failed: {e}"))
                            })?;
                        }
                        2 => {
                            let mut out = io::stderr();
                            out.write_all(&bytes).await.map_err(|e| {
                                EngineError::Internal(format!("std.io.write_all failed: {e}"))
                            })?;
                            out.flush().await.map_err(|e| {
                                EngineError::Internal(format!("std.io.write_all failed: {e}"))
                            })?;
                        }
                        _ => {
                            return Err(EngineError::Internal(format!(
                                "std.io.write_all only supports fd 1 (stdout) and 2 (stderr), got {fd}"
                            )));
                        }
                    }
                    ().into_rex(ctx.heap())
                }
                .boxed()
            })
        },
    )?;

    builder.inject_module(module)
}

fn io_typeclass_decls() -> Result<Vec<rex::ast::Decl>, EngineError> {
    let source = r#"
        instance Functor IO where {
            map = io_map;
        }

        instance Applicative IO <= Functor IO where {
            pure = io_pure;
            ap = io_ap;
        }

        instance Monad IO <= Applicative IO where {
            bind = io_bind;
        }
    "#;
    parse_rex(source)
        .map(|program| program.decls)
        .map_err(|errs| {
            EngineError::Internal(format!("failed to parse std.io declarations: {errs:?}"))
        })
}

fn io_adt_decl() -> AdtDecl {
    let mut supply = TypeVarSupply::new();
    let mut adt = AdtDecl::new(&Symbol::intern("IO"), &[Symbol::intern("a")], &mut supply);
    adt.add_variant(Symbol::intern("IO"), vec![uuid_type()]);
    adt
}

fn alloc_io_action(ctx: &Context<()>, action: IoAction) -> Result<Handle, EngineError> {
    let id = Uuid::new_v4();
    io_registry()
        .actions
        .lock()
        .map_err(|_| EngineError::Internal("std.io action registry mutex poisoned".into()))?
        .insert(id, action);
    let id = id.into_rex(ctx.heap())?;
    ctx.heap().alloc_adt(Symbol::intern("IO"), vec![id])
}

pub fn io_result_type_arg(typ: &Type) -> Option<Type> {
    io_type_arg(typ)
}

pub async fn run_io_handle(ctx: Context<()>, action: Handle) -> Result<Handle, EngineError> {
    run_host_action(ctx, action, lookup_io_action).await
}

fn lookup_io_action(action: &Handle) -> Result<IoAction, EngineError> {
    let id = io_action_id(action)?;
    io_registry()
        .actions
        .lock()
        .map_err(|_| EngineError::Internal("std.io action registry mutex poisoned".into()))?
        .get(&id)
        .cloned()
        .ok_or_else(|| EngineError::Internal(format!("std.io: unknown IO action {id}")))
}

fn io_action_id(action: &Handle) -> Result<Uuid, EngineError> {
    let (tag, args) = action.as_adt()?;
    if tag.as_ref() != "IO" || args.len() != 1 {
        return Err(EngineError::NativeType {
            expected: "IO".into(),
            got: action.type_name()?.into(),
        });
    }
    Uuid::from_rex(&args[0])
}

fn export_io0<F>(
    module: &mut Module<()>,
    name: &'static str,
    ret: Type,
    build: F,
) -> Result<(), EngineError>
where
    F: Fn(Context<()>) -> IoEffect + Send + Sync + 'static,
{
    module.export_native(
        name,
        Scheme::new(vec![], vec![], ret),
        0,
        move |ctx, _typ, _args| {
            let action = build(ctx.clone());
            alloc_io_action(&ctx, IoAction::Effect(action))
        },
    )
}

fn export_io1<A, F>(
    module: &mut Module<()>,
    name: &'static str,
    arg: Type,
    ret: Type,
    build: F,
) -> Result<(), EngineError>
where
    A: FromRex + Send + Sync + 'static,
    F: Fn(A) -> IoEffect + Send + Sync + 'static,
{
    module.export_native(
        name,
        Scheme::new(vec![], vec![], Type::fun(arg, io_of(ret))),
        1,
        move |ctx, _typ, args| {
            let arg = args
                .first()
                .ok_or_else(|| EngineError::Internal(format!("std.io.{name} missing argument")))?;
            let action = build(A::from_rex(arg)?);
            alloc_io_action(&ctx, IoAction::Effect(action))
        },
    )
}

fn export_io2<A, B, F>(
    module: &mut Module<()>,
    name: &'static str,
    arg_a: Type,
    arg_b: Type,
    ret: Type,
    build: F,
) -> Result<(), EngineError>
where
    A: FromRex + Send + Sync + 'static,
    B: FromRex + Send + Sync + 'static,
    F: Fn(A, B) -> IoEffect + Send + Sync + 'static,
{
    module.export_native(
        name,
        Scheme::new(
            vec![],
            vec![],
            Type::fun(arg_a, Type::fun(arg_b, io_of(ret))),
        ),
        2,
        move |ctx, _typ, args| {
            let arg_a = args.first().ok_or_else(|| {
                EngineError::Internal(format!("std.io.{name} missing first argument"))
            })?;
            let arg_b = args.get(1).ok_or_else(|| {
                EngineError::Internal(format!("std.io.{name} missing second argument"))
            })?;
            let action = build(A::from_rex(arg_a)?, B::from_rex(arg_b)?);
            alloc_io_action(&ctx, IoAction::Effect(action))
        },
    )
}

fn split_fun_chain(typ: &Type, arity: usize) -> Result<(Vec<Type>, Type), EngineError> {
    let mut args = Vec::with_capacity(arity);
    let mut cur = typ.clone();
    for _ in 0..arity {
        let TypeKind::Fun(arg, ret) = cur.as_ref() else {
            return Err(EngineError::NotCallable(cur.to_string()));
        };
        args.push(arg.clone());
        cur = ret.clone();
    }
    Ok((args, cur))
}

fn io_type_arg(typ: &Type) -> Option<Type> {
    let TypeKind::App(head, arg) = typ.as_ref() else {
        return None;
    };
    let TypeKind::Con(con) = head.as_ref() else {
        return None;
    };
    let name = con.name();
    (con.arity() == 1 && name.as_ref().rsplit('.').next() == Some("IO")).then(|| arg.clone())
}

fn named_var(supply: &mut TypeVarSupply, name: &'static str) -> TypeVar {
    supply.fresh(Some(Symbol::intern(name)))
}

fn io_pure_scheme() -> Scheme {
    let mut supply = TypeVarSupply::new();
    let a = named_var(&mut supply, "a");
    let a_ty = Type::var(a.clone());
    Scheme::new(vec![a], vec![], Type::fun(a_ty.clone(), io_of(a_ty)))
}

fn io_map_scheme() -> Scheme {
    let mut supply = TypeVarSupply::new();
    let a = named_var(&mut supply, "a");
    let b = named_var(&mut supply, "b");
    let a_ty = Type::var(a.clone());
    let b_ty = Type::var(b.clone());
    Scheme::new(
        vec![a, b],
        vec![],
        Type::fun(
            Type::fun(a_ty.clone(), b_ty.clone()),
            Type::fun(io_of(a_ty), io_of(b_ty)),
        ),
    )
}

fn io_ap_scheme() -> Scheme {
    let mut supply = TypeVarSupply::new();
    let a = named_var(&mut supply, "a");
    let b = named_var(&mut supply, "b");
    let a_ty = Type::var(a.clone());
    let b_ty = Type::var(b.clone());
    Scheme::new(
        vec![a, b],
        vec![],
        Type::fun(
            io_of(Type::fun(a_ty.clone(), b_ty.clone())),
            Type::fun(io_of(a_ty), io_of(b_ty)),
        ),
    )
}

fn io_bind_scheme() -> Scheme {
    let mut supply = TypeVarSupply::new();
    let a = named_var(&mut supply, "a");
    let b = named_var(&mut supply, "b");
    let a_ty = Type::var(a.clone());
    let b_ty = Type::var(b.clone());
    Scheme::new(
        vec![a, b],
        vec![],
        Type::fun(
            Type::fun(a_ty.clone(), io_of(b_ty.clone())),
            Type::fun(io_of(a_ty), io_of(b_ty)),
        ),
    )
}

fn io_of(inner: Type) -> Type {
    Type::app(Type::user_con("IO", 1), inner)
}

fn array_of(inner: Type) -> Type {
    Type::app(Type::builtin(BuiltinTypeId::Array), inner)
}

fn unit_type() -> Type {
    Type::tuple(Vec::<Type>::new())
}

fn bool_type() -> Type {
    Type::builtin(BuiltinTypeId::Bool)
}

fn u8_type() -> Type {
    Type::builtin(BuiltinTypeId::U8)
}

fn i32_type() -> Type {
    Type::builtin(BuiltinTypeId::I32)
}

fn i64_type() -> Type {
    Type::builtin(BuiltinTypeId::I64)
}

fn string_type() -> Type {
    Type::builtin(BuiltinTypeId::String)
}

fn uuid_type() -> Type {
    Type::builtin(BuiltinTypeId::Uuid)
}
