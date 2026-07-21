use std::sync::Arc;

use futures::future::BoxFuture;
use rex_typesystem::types::Type;

use crate::{error::EngineError, evaluator::context::Context, memory::heap::Handle};

/// Boxed future returned by a host-managed action effect.
pub type HostActionFuture = BoxFuture<'static, Result<Handle, EngineError>>;

/// Effect thunk used by host-managed action runners.
pub type HostActionEffect<State> =
    Arc<dyn Fn(Context<State>) -> HostActionFuture + Send + Sync + 'static>;

/// One node in a host-managed monadic action graph.
///
/// This lets embedders expose Haskell-style action values without exposing a
/// general "apply arbitrary Rex function" API. The engine owns callback
/// application while the host owns action lookup and effects.
#[derive(Clone)]
pub enum HostAction<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    /// An action that has already produced a Rex value.
    Pure(Handle),
    /// A host effect to run at the action boundary.
    Effect(HostActionEffect<State>),
    /// Apply a pure Rex callback to the result of another action.
    Map {
        /// Rex callback.
        f: Handle,
        /// Rex type of `f`.
        f_type: Type,
        /// Rex type produced by `action`.
        input_type: Type,
        /// Action whose value is passed to `f`.
        action: Handle,
    },
    /// Applicative function application.
    Ap {
        /// Action that produces the Rex callback.
        f_action: Handle,
        /// Action that produces the Rex argument.
        action: Handle,
        /// Rex type of the callback.
        f_type: Type,
        /// Rex type produced by `action`.
        input_type: Type,
    },
    /// Monadic bind.
    Bind {
        /// Rex callback producing the next action.
        f: Handle,
        /// Rex type of `f`.
        f_type: Type,
        /// Rex type produced by `action`.
        input_type: Type,
        /// Action whose value is passed to `f`.
        action: Handle,
    },
}

enum HostContinuation {
    Map {
        f: Handle,
        f_type: Type,
        input_type: Type,
    },
    Bind {
        f: Handle,
        f_type: Type,
        input_type: Type,
    },
    ApRunArg {
        action: Handle,
        f_type: Type,
        input_type: Type,
    },
}

enum HostRunnerStep {
    Next(Handle),
    Done(Handle),
}

/// Run one host-managed action graph with an explicit continuation stack.
///
/// `lookup` maps the opaque Rex action handle to the next host action node.
/// Rex callbacks inside `map`, `ap`, and `bind` are evaluated by the engine one
/// step at a time; the runner never recursively awaits itself.
pub async fn run_host_action<State, Lookup>(
    ctx: Context<State>,
    action: Handle,
    mut lookup: Lookup,
) -> Result<Handle, EngineError>
where
    State: Clone + Send + Sync + 'static,
    Lookup: FnMut(&Handle) -> Result<HostAction<State>, EngineError>,
{
    let mut current = action;
    let mut continuations = Vec::new();

    loop {
        match lookup(&current)? {
            HostAction::Pure(value) => {
                match continue_host_action_value(&ctx, value, &mut continuations).await? {
                    HostRunnerStep::Next(next) => current = next,
                    HostRunnerStep::Done(value) => return Ok(value),
                }
            }
            HostAction::Effect(effect) => {
                let value = effect(ctx.clone()).await?;
                match continue_host_action_value(&ctx, value, &mut continuations).await? {
                    HostRunnerStep::Next(next) => current = next,
                    HostRunnerStep::Done(value) => return Ok(value),
                }
            }
            HostAction::Map {
                f,
                f_type,
                input_type,
                action,
            } => {
                continuations.push(HostContinuation::Map {
                    f,
                    f_type,
                    input_type,
                });
                current = action;
            }
            HostAction::Ap {
                f_action,
                action,
                f_type,
                input_type,
            } => {
                continuations.push(HostContinuation::ApRunArg {
                    action,
                    f_type,
                    input_type,
                });
                current = f_action;
            }
            HostAction::Bind {
                f,
                f_type,
                input_type,
                action,
            } => {
                continuations.push(HostContinuation::Bind {
                    f,
                    f_type,
                    input_type,
                });
                current = action;
            }
        }
    }
}

async fn continue_host_action_value<State>(
    ctx: &Context<State>,
    mut value: Handle,
    continuations: &mut Vec<HostContinuation>,
) -> Result<HostRunnerStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    loop {
        match continuations.pop() {
            None => return Ok(HostRunnerStep::Done(value)),
            Some(HostContinuation::Map {
                f,
                f_type,
                input_type,
            }) => {
                value = ctx
                    .resume_callback_once(f, f_type, vec![(value, input_type)])
                    .await?;
            }
            Some(HostContinuation::Bind {
                f,
                f_type,
                input_type,
            }) => {
                return ctx
                    .resume_callback_once(f, f_type, vec![(value, input_type)])
                    .await
                    .map(HostRunnerStep::Next);
            }
            Some(HostContinuation::ApRunArg {
                action,
                f_type,
                input_type,
            }) => {
                continuations.push(HostContinuation::Map {
                    f: value,
                    f_type,
                    input_type,
                });
                return Ok(HostRunnerStep::Next(action));
            }
        }
    }
}
