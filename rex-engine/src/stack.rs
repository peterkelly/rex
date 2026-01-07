use std::thread;

use crate::EngineError;

pub const DEFAULT_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

pub fn run_with_stack_size<R>(
    stack_size: usize,
    f: impl FnOnce() -> R + Send,
) -> Result<R, EngineError>
where
    R: Send,
{
    thread::scope(|scope| {
        let handle = thread::Builder::new()
            .name("rex-engine".to_string())
            .stack_size(stack_size)
            .spawn_scoped(scope, f)
            .map_err(|e| EngineError::Internal(format!("failed to spawn worker thread: {e}")))?;
        handle
            .join()
            .map_err(|_| EngineError::Internal("worker thread panicked".into()))
    })
}
